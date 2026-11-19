//! getComputedStyle of `border-radius` keeps a percentage and a `calc()`
//! mix as the computed value (css-backgrounds-3 §5.1: computed value "pair
//! of computed <length-percentage> values"). Every radius was resolved to
//! px against the box, so `50%` on a 200x100 box read `100px 50px`.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/border_radius_computed_log.mjs`, receipt
//! `docs/diagnostics/2026-11-17/border_radius_computed_cft148.json`: a
//! 200x100 box with 16px text, an 800x600 viewport; the shorthand and the
//! four corners.

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// `shorthand | top-left | top-right | bottom-right | bottom-left`.
fn read(css: &str) -> String {
    let page = format!(
        "<!doctype html><style>html{{font-size:16px}}body{{margin:0}}\
         #b{{width:200px;height:100px;font:16px sans-serif;{css}}}</style><div id=b></div>"
    );
    let server = serve_routes(vec![("/", "text/html", page)]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    let value = engine
        .execute_script(
            view,
            "var c=getComputedStyle(document.getElementById('b'));\
             String(['border-radius','border-top-left-radius','border-top-right-radius',\
             'border-bottom-right-radius','border-bottom-left-radius']\
             .map(function(p){return c.getPropertyValue(p)}).join(' | '))",
        )
        .unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

#[track_caller]
fn check(css: &str, chrome: &str) {
    assert_eq!(read(css), chrome, "{css}");
}

#[test]
fn percentages_stay_percentages() {
    check("border-radius:50%", "50% | 50% | 50% | 50% | 50%");
    check("border-radius:80%", "80% | 80% | 80% | 80% | 80%");
    check(
        "border-radius:10% / 20%",
        "10% / 20% | 10% 20% | 10% 20% | 10% 20% | 10% 20%",
    );
    check(
        "border-radius:10px 50% 2em 1rem",
        "10px 50% 32px 16px | 10px | 50% | 32px | 16px",
    );
    check(
        "border-top-left-radius:10px 20%",
        "10px 0px 0px / 20% 0px 0px | 10px 20% | 0px | 0px | 0px",
    );
}

#[test]
fn calc_mixes_stay_calc() {
    check(
        "border-radius:calc(10px + 5%)",
        "calc(5% + 10px) | calc(5% + 10px) | calc(5% + 10px) | calc(5% + 10px) | calc(5% + 10px)",
    );
    check(
        "border-top-left-radius:calc(50% - 10px)",
        "calc(50% - 10px) 0px 0px | calc(50% - 10px) | 0px | 0px | 0px",
    );
    check(
        "border-top-left-radius:calc(1em + 10%)",
        "calc(10% + 16px) 0px 0px | calc(10% + 16px) | 0px | 0px | 0px",
    );
}

/// Lengths still compute to px (these read right before too).
#[test]
fn lengths_compute_to_px() {
    check("border-radius:10px", "10px | 10px | 10px | 10px | 10px");
    check(
        "border-radius:2em / 1.5rem",
        "32px / 24px | 32px 24px | 32px 24px | 32px 24px | 32px 24px",
    );
    check(
        "border-top-left-radius:calc(2 * 10px)",
        "20px 0px 0px | 20px | 0px | 0px | 0px",
    );
}

/// A stated gap (handoff row 3): computed reads have no viewport, so a
/// `vw` radius reads `0px` (Chrome 148: `80px` in an 800px viewport).
#[test]
#[ignore = "computed reads have no viewport for vw / vh; open row"]
fn a_vw_radius_computes_to_px() {
    check(
        "border-top-left-radius:10vw",
        "80px 0px 0px | 80px | 0px | 0px | 0px",
    );
}
