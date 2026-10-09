//! The page is laid out again between the lifecycle steps (see
//! `Engine::refresh_layout_for_script`), so a handler that runs after a
//! script's writes reads the geometry those writes made.

use super::*;
use crate::script_net_tests::serve_routes;

fn read(engine: &mut Engine, view: EngineViewId, name: &str) -> String {
    let value = engine.execute_script(view, &format!("String(window.{name})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

#[test]
fn lifecycle_handlers_read_geometry_after_the_previous_steps_writes() {
    let page = "<html><head><style>body{margin:0}</style></head><body>\
        <div id=a style='width:100px;height:20px'></div>\
        <script>\
        window.__during = document.getElementById('a').offsetHeight;\
        document.getElementById('a').style.height = '80px';\
        document.addEventListener('DOMContentLoaded', function () {\
          window.__dcl = document.getElementById('a').offsetHeight;\
          document.getElementById('a').style.height = '120px';\
        });\
        window.addEventListener('load', function () {\
          window.__load = document.getElementById('a').offsetHeight;\
          document.getElementById('a').style.height = '160px';\
        });\
        setTimeout(function () {\
          window.__timer = document.getElementById('a').getBoundingClientRect().height;\
        }, 0);\
        </script></body></html>";
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    // Same task as the write: still the old layout (a documented limit).
    assert_eq!(read(&mut engine, view, "__during"), "20");
    assert_eq!(read(&mut engine, view, "__dcl"), "80", "DOMContentLoaded sees the script's write");
    assert_eq!(read(&mut engine, view, "__load"), "120", "load sees the DOMContentLoaded handler's write");
    assert_eq!(read(&mut engine, view, "__timer"), "160", "a timer sees the load handler's write");
}
