//! Visually hidden ("screen-reader-only") boxes (Z row S5): the legacy
//! `clip: rect()` property and `clip-path: inset(50%)` on a 1x1 absolute box.
//! Before this row `clip` was not parsed at all, so eBay's `.clipped` header
//! labels ("expand Cart") and Bootstrap's `.visually-hidden` text painted.
//!
//! Each row is Chromium 143.0.7499.4's ink for the hand-made body: the number of
//! pixels that are not pure white in a 400x300 frame. The hidden box is red
//! with red 20px text, so anything that leaks is ink. Written by
//! `trench/z/tools/s5/clip_gen.py` on the Atlas hub (probe `clip_probe.mjs`).

use super::*;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;

const CASES: &[(&str, &str, usize)] = &[
    ("control: abs 1x1 overflow hidden, no clip", r##"<span style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;top:50px;left:50px">expand Cart</span>"##, 1),
    ("control: abs 40x40 block", r##"<div style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px"></div>"##, 1600),
    ("ebay clipped: clip rect 1px x4 + clip-path inset 50%", r##"<span style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;top:50px;left:50px;clip:rect(1px,1px,1px,1px);clip-path:inset(50%)">expand Cart</span>"##, 0),
    ("bootstrap: clip rect(0,0,0,0) margin -1px", r##"<span style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;top:50px;left:50px;margin:-1px;clip:rect(0,0,0,0)">expand Cart</span>"##, 0),
    ("clip rect(0 0 0 0) space form", r##"<span style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;top:50px;left:50px;clip:rect(0 0 0 0)">expand Cart</span>"##, 0),
    ("clip only, no clip-path", r##"<span style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;top:50px;left:50px;clip:rect(1px,1px,1px,1px)">expand Cart</span>"##, 0),
    ("clip-path inset(50%) only", r##"<span style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;top:50px;left:50px;clip-path:inset(50%)">expand Cart</span>"##, 0),
    ("clip-path inset(100%) only", r##"<span style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;top:50px;left:50px;clip-path:inset(100%)">expand Cart</span>"##, 0),
    ("clip rect no overflow", r##"<span style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:1px;height:1px;white-space:nowrap;clip:rect(0,0,0,0)">expand Cart</span>"##, 0),
    ("clip-path inset 50% no overflow", r##"<span style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:1px;height:1px;white-space:nowrap;clip-path:inset(50%)">expand Cart</span>"##, 0),
    ("clip rect on 40x40 block", r##"<div style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;clip:rect(10px,30px,30px,10px)"></div>"##, 400),
    ("clip rect auto sides", r##"<div style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;clip:rect(auto,20px,auto,auto)"></div>"##, 800),
    ("clip rect on static box: ignored", r##"<div style="background:red;color:red;font:20px/20px Arial;width:40px;height:40px;clip:rect(0,0,0,0)"></div>"##, 1600),
    ("clip auto", r##"<div style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;clip:auto"></div>"##, 1600),
    ("clip on fixed box", r##"<div style="background:red;color:red;font:20px/20px Arial;position:fixed;top:50px;left:50px;width:40px;height:40px;clip:rect(0,0,0,0)"></div>"##, 0),
    ("clip rect with child overflowing", r##"<div style="position:absolute;top:50px;left:50px;width:40px;height:40px;clip:rect(0,20px,20px,0)"><div style="width:80px;height:80px;background:red"></div></div>"##, 400),
    ("in a link inside a header", r##"<header style="height:60px"><a href="#" style="display:inline-block;width:20px;height:20px"><span style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;clip:rect(1px,1px,1px,1px);clip-path:inset(50%)">expand Cart</span></a></header>"##, 0),
    ("button clipped (ebay)", r##"<button style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;top:50px;left:50px;clip:rect(1px,1px,1px,1px);clip-path:inset(50%)">expand Cart</button>"##, 0),
    ("button clip-path only", r##"<button style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;top:50px;left:50px;clip-path:inset(50%)">expand Cart</button>"##, 0),
    ("button clip only", r##"<button style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;top:50px;left:50px;clip:rect(0,0,0,0)">expand Cart</button>"##, 0),
    ("input clip-path only", r##"<input value="expand Cart" style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;top:50px;left:50px;clip-path:inset(50%)">"##, 0),
    ("in a flex row", r##"<div style="display:flex;height:60px"><span style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;clip-path:inset(50%)">expand Cart</span><span>&nbsp;</span></div>"##, 0),
    ("in a flex row, clip only", r##"<div style="display:flex;height:60px"><span style="background:red;color:red;font:20px/20px Arial;position:absolute;width:1px;height:1px;overflow:hidden;white-space:nowrap;border:0;padding:0;clip:rect(0,0,0,0)">expand Cart</span></div>"##, 0),
    ("inline-block clip-path inset 50%", r##"<span style="background:red;color:red;font:20px/20px Arial;display:inline-block;position:absolute;top:50px;left:50px;width:1px;height:1px;white-space:nowrap;clip-path:inset(50%)">expand Cart</span>"##, 0),
    ("abs child of a clipped box", r##"<div style="position:absolute;top:50px;left:50px;width:1px;height:1px;clip:rect(0,0,0,0)"><div style="position:absolute;width:40px;height:40px;background:red"></div></div>"##, 0),
];

fn ink(engine: &mut Engine, view: EngineViewId) -> usize {
    engine.render_view(view).expect("render");
    let path = std::env::temp_dir().join(format!(
        "rustkit-s5-clip-{}-{:?}.ppm",
        std::process::id(),
        std::thread::current().id()
    ));
    engine
        .capture_frame(view, path.to_str().unwrap())
        .expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm[ppm.len() - WIDTH as usize * HEIGHT as usize * 3..]
        .chunks_exact(3)
        .filter(|p| *p != [255, 255, 255])
        .count()
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn visually_hidden_boxes_paint_as_chromium() {
    let mut wrong = Vec::new();
    for (name, body, want) in CASES {
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds::new(0, 0, WIDTH, HEIGHT))
            .expect("headless view");
        let html = format!(
            "<!doctype html><style>html,body{{margin:0;background:#fff}}div{{box-sizing:border-box}}</style>{body}"
        );
        engine.load_html(view, &html).expect("load_html");
        let got = ink(&mut engine, view);
        if got != *want {
            wrong.push(format!("{name}: ink {got}, Chromium {want}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} wrong:\n{}",
        wrong.len(),
        CASES.len(),
        wrong.join("\n")
    );
}

/// eBay's header (Z row S5, reopened): the "Expand Cart / Watch List / My
/// eBay / Sell" buttons and the "Skip to main content" link hide with
/// `clip-path: polygon(0 0,0 0)`, a polygon with no area, shown only on
/// `:focus`. Same ink metric; Chromium 143.0.7499.4 rows from the S5b
/// Playwright probe (the clip_probe.mjs method, polygon cases).
const POLYGON_CASES: &[(&str, &str, usize)] = &[
    ("ebay a11y button: polygon(0 0,0 0)", r##"<div style="position:relative;height:60px"><a href="#" style="display:inline-block;width:20px;height:20px"></a><button style="background:red;color:red;font:20px/20px Arial;border-radius:8px;clip-path:polygon(0 0,0 0);display:block;left:0;padding:4px;position:absolute;white-space:nowrap;z-index:-1;border:0">Expand Cart</button></div>"##, 0),
    ("ebay a11y button without z-index", r##"<div style="position:relative;height:60px"><button style="background:red;color:red;font:20px/20px Arial;border-radius:8px;clip-path:polygon(0 0,0 0);display:block;left:0;top:10px;padding:4px;position:absolute;white-space:nowrap;border:0">Expand Cart</button></div>"##, 0),
    ("ebay skip link: static link in 1x1 visible wrapper", r##"<div style="display:block;height:1px;overflow:visible;position:absolute;width:1px;top:20px;left:20px"><a href="#main" style="background:red;color:red;font:20px/20px Arial;border-radius:8px;clip-path:polygon(0 0,0 0);display:block;padding:4px;position:static;text-decoration:none;white-space:nowrap;z-index:-1">Skip to main content</a></div>"##, 0),
    ("span polygon(0 0,0 0)", r##"<span style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;clip-path:polygon(0 0,0 0)">expand Cart</span>"##, 0),
    ("polygon one point", r##"<div style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;clip-path:polygon(50% 50%)"></div>"##, 0),
    ("polygon collinear line", r##"<div style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;clip-path:polygon(0 0,100% 100%,50% 50%)"></div>"##, 0),
    ("polygon evenodd degenerate", r##"<div style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;clip-path:polygon(evenodd,0 0,0 0)"></div>"##, 0),
    ("polygon full box", r##"<div style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;clip-path:polygon(0 0,100% 0,100% 100%,0 100%)"></div>"##, 1600),
    ("polygon inner rect px", r##"<div style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;clip-path:polygon(10px 10px,30px 10px,30px 30px,10px 30px)"></div>"##, 400),
    ("polygon inner rect pct, reverse winding", r##"<div style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;clip-path:polygon(25% 25%,25% 75%,75% 75%,75% 25%)"></div>"##, 400),
    ("polygon rect with child overflowing", r##"<div style="position:absolute;top:50px;left:50px;width:40px;height:40px;clip-path:polygon(0 0,20px 0,20px 20px,0 20px)"><div style="width:80px;height:80px;background:red"></div></div>"##, 400),
    ("polygon padding-box keyword", r##"<div style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;border:5px solid red;clip-path:polygon(0 0,100% 0,100% 100%,0 100%) padding-box"></div>"##, 900),
    ("input polygon(0 0,0 0)", r##"<input value="expand Cart" style="background:red;color:red;font:20px/20px Arial;position:absolute;top:50px;left:50px;width:40px;height:40px;clip-path:polygon(0 0,0 0)">"##, 0),
];

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn clip_path_polygon_hides_as_chromium() {
    let mut wrong = Vec::new();
    for (name, body, want) in POLYGON_CASES {
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds::new(0, 0, WIDTH, HEIGHT))
            .expect("headless view");
        let html = format!(
            "<!doctype html><style>html,body{{margin:0;background:#fff}}div{{box-sizing:border-box}}</style>{body}"
        );
        engine.load_html(view, &html).expect("load_html");
        let got = ink(&mut engine, view);
        if got != *want {
            wrong.push(format!("{name}: ink {got}, Chromium {want}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} wrong:\n{}",
        wrong.len(),
        POLYGON_CASES.len(),
        wrong.join("\n")
    );
}
