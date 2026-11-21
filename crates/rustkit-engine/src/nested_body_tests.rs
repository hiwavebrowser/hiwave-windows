//! S11: a second `<body>` start tag (gmail's sign-in page) was nested inside
//! the first. The inner body kept the UA stylesheet's `margin: 8px`, so the
//! page root sat at x=8, 1264 wide, in a 1280 window. Per HTML §13.2.6.4.7
//! the second tag only merges attributes onto the existing body.

use super::*;
use crate::script_net_tests::serve_routes;

fn measured(page: &str, script: &str) -> String {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 1280, height: 800 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    engine.execute_script(view, script).expect("script")
}

const ROOT: &str = "var r = document.getElementById('root').getBoundingClientRect(); \
     document.getElementsByTagName('body').length + ' ' + r.x + ' ' + r.width + ' ' + \
     document.body.className + ' ' + document.body.id";

/// The reduced gmail shape: the first body zeroes its margin, a second
/// body tag follows. One body, the root at x=0 and the window's width.
#[test]
fn a_second_body_tag_does_not_nest_a_body_with_the_ua_margin() {
    let page = "<!DOCTYPE html><html><head></head><body style=\"margin:0\" class=a>\
                <body class=b id=x><div id=root>x</div></body></html>";
    assert_eq!(measured(page, ROOT), "String(\"1 0 1280 a x\")");
}
