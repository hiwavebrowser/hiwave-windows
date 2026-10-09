//! The "sticky footer" page: `body { min-height: 100vh; display: flex;
//! flex-direction: column }` with `main { flex: 1; overflow: hidden }`.
//! `flex: 1` is a flex basis of `0%`, and a percentage basis in a container
//! whose main size is indefinite is `content` (css-flexbox-1 §7.2.3), so
//! the item is as tall as what it holds and the body grows past the
//! viewport. Until 2026-10-08 the basis was taken as 0 and the item, which
//! may clip, got only the space the viewport had left: on
//! simonwillison.net 4388px of posts sat in a 183px box and the page had
//! nothing to scroll (hand test, "scrolling").

use super::*;
use crate::script_net_tests::serve_routes;

fn load(page: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn page(wrapper: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><style>body{{margin:0;min-height:100vh;display:flex;flex-direction:column}}\
         #head{{height:30px}} #w{{{wrapper}}} #tall{{height:1000px}} #foot{{height:20px}}</style></head>\
         <body><div id=head></div><div id=w><div id=tall></div></div><div id=foot></div></body></html>"
    )
}

/// Heights from Chromium 143 (the oracle) on these pages at 400 x 200.
#[test]
fn a_percentage_basis_in_an_auto_height_column_is_content() {
    for (wrapper, height) in [
        ("flex:1;overflow:hidden", "1000"),
        ("flex:1", "1000"),
        ("flex:1 1 0%;overflow:hidden", "1000"),
        ("flex:0 1 50%;overflow:hidden", "1000"),
        ("flex:1 1;overflow:hidden", "1000"),
        ("flex-grow:1;flex-basis:0%;overflow:hidden", "1000"),
    ] {
        let (mut engine, view, _server) = load(&page(wrapper));
        assert_eq!(
            read(&mut engine, view, "document.getElementById('w').offsetHeight"),
            height,
            "#w {{ {wrapper} }}"
        );
        assert_eq!(read(&mut engine, view, "document.getElementById('foot').offsetTop"), "1030", "{wrapper}");
        // The window scrolls to the footer's end: 1050 of content in 200.
        assert_eq!(engine.views[&view].max_scroll_offset.1, 850.0, "{wrapper}");
        assert!(engine.scroll_view(view, 0.0, -40.0).unwrap(), "the page scrolls ({wrapper})");
    }
}

/// A short page still fills the viewport: the item takes the free space.
#[test]
fn a_short_sticky_footer_page_still_fills_the_viewport() {
    let short = page("flex:1;overflow:hidden").replace("#tall{height:1000px}", "#tall{height:40px}");
    let (mut engine, view, _server) = load(&short);
    assert_eq!(read(&mut engine, view, "document.getElementById('w').offsetHeight"), "150");
    assert_eq!(read(&mut engine, view, "document.getElementById('foot').offsetTop"), "180");
    assert_eq!(engine.views[&view].max_scroll_offset.1, 0.0);
}

/// A definite-height column resolves the percentage: nothing changes there.
#[test]
fn a_percentage_basis_in_a_definite_height_column_is_resolved() {
    let fixed = page("flex:1;overflow:hidden").replace("min-height:100vh", "height:200px");
    let (mut engine, view, _server) = load(&fixed);
    assert_eq!(read(&mut engine, view, "document.getElementById('w').offsetHeight"), "150");
}

/// A basis written as a length is a length: `0` and `0px` are not `0%`.
#[test]
fn a_zero_length_basis_in_an_auto_height_column_stays_zero() {
    for wrapper in ["flex:1 1 0;overflow:hidden", "flex:1 1 0px;overflow:hidden"] {
        let (mut engine, view, _server) = load(&page(wrapper));
        assert_eq!(read(&mut engine, view, "document.getElementById('w').offsetHeight"), "150", "{wrapper}");
        assert_eq!(read(&mut engine, view, "document.getElementById('foot').offsetTop"), "180", "{wrapper}");
    }
}
