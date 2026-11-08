//! CH3.A7: the app bridge (`window.ipc`, `window.hiwaveChrome` and the
//! host function behind them, `__rk_ipc_post`) is installed only in a
//! document the app owns (the chrome, the shelf, the Inspector, the
//! settings page), never in web content. Chromium 143 gives a page none of
//! them: `typeof window.ipc`, `typeof window.hiwaveChrome` and
//! `typeof __rk_ipc_post` are all `"undefined"` there.
//!
//! A page engine (no presenter, no GPU surface), as the content thread
//! builds it.

use super::*;
use crate::script_net_tests::serve_routes;

const BRIDGE: &str = "[typeof window.ipc, typeof window.hiwaveChrome, typeof __rk_ipc_post, \
     'ipc' in window, 'hiwaveChrome' in window].join(',')";

/// What Chromium 143 answers for [`BRIDGE`] on any web page.
const CHROMIUM: &str = "undefined,undefined,undefined,false,false";

fn page_engine() -> (Engine, EngineViewId) {
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0)))
        .expect("engine");
    let id = engine
        .create_page_view(Bounds::new(0, 0, 800, 600))
        .expect("page view");
    (engine, id)
}

fn js(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let out = engine
        .execute_script(id, &format!("String(eval({script:?}))"))
        .expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .map(|s| s.replace("\\\"", "\""))
        .unwrap_or(out)
}

fn load_url(engine: &mut Engine, id: EngineViewId, body: &str) {
    let server = serve_routes(vec![("/", "text/html", body.to_string())]);
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(id, url)).expect("load_url");
}

#[test]
fn a_web_document_loaded_from_html_sees_no_app_bridge() {
    let (mut engine, id) = page_engine();
    engine.load_html(id, "<p>web</p>").unwrap();
    assert_eq!(js(&mut engine, id, BRIDGE), CHROMIUM);
}

#[test]
fn a_web_document_loaded_from_a_url_sees_no_app_bridge() {
    let (mut engine, id) = page_engine();
    load_url(&mut engine, id, "<p>web</p>");
    assert_eq!(js(&mut engine, id, BRIDGE), CHROMIUM);
}

#[test]
fn a_web_page_s_script_sees_no_app_bridge_while_it_loads() {
    let (mut engine, id) = page_engine();
    engine
        .load_html(
            id,
            "<p id=out></p><script>document.getElementById('out').textContent = \
             typeof window.ipc + ',' + typeof window.hiwaveChrome;</script>",
        )
        .unwrap();
    assert_eq!(
        js(&mut engine, id, "document.getElementById('out').textContent"),
        "undefined,undefined"
    );
}

#[test]
fn an_app_owned_document_keeps_window_ipc_and_its_messages_reach_the_engine() {
    let (mut engine, id) = page_engine();
    engine.set_app_owned(id, true);
    engine
        .load_html(
            id,
            "<p>chrome</p><script>window.ipc.postMessage(JSON.stringify({cmd: 'get_sidebar_width'}));\
             window.hiwaveChrome.postMessage({cmd: 'get_version'});</script>",
        )
        .unwrap();
    assert_eq!(
        js(&mut engine, id, "typeof window.ipc.postMessage + ',' + typeof window.hiwaveChrome.postMessage"),
        "function,function"
    );
    let got: Vec<String> = engine.drain_view_ipc(id).into_iter().map(|m| m.payload).collect();
    assert_eq!(
        got,
        vec![r#"{"cmd":"get_sidebar_width"}"#.to_string(), r#"{"cmd":"get_version"}"#.to_string()]
    );
}

/// The bridge belongs to the app's documents, not to the view: an app view
/// that ends up on a web URL has none.
#[test]
fn an_app_owned_view_navigated_to_the_web_gives_that_page_no_bridge() {
    let (mut engine, id) = page_engine();
    engine.set_app_owned(id, true);
    load_url(&mut engine, id, "<p>web</p>");
    assert_eq!(js(&mut engine, id, BRIDGE), CHROMIUM);
}

/// Whatever a view no longer owned by the app had queued is dropped, not
/// handed to the app's command handler.
#[test]
fn the_engine_ignores_ipc_from_a_view_the_app_does_not_own() {
    let (mut engine, id) = page_engine();
    engine.set_app_owned(id, true);
    engine
        .load_html(id, "<script>window.ipc.postMessage('{\"cmd\":\"navigate\"}');</script>")
        .unwrap();
    engine.set_app_owned(id, false);
    assert!(engine.drain_view_ipc(id).is_empty());
    assert!(engine.drain_ipc_messages().is_empty());
}

#[test]
fn views_are_not_app_owned_by_default() {
    let (engine, id) = page_engine();
    assert!(!engine.is_app_owned(id));
}

/// What the log keeps of an ignored message: the `cmd` when it is a plain
/// name, never the rest of the body.
#[test]
fn an_ignored_message_is_logged_by_its_cmd_shape_only() {
    assert_eq!(ipc_cmd_shape(r#"{"cmd":"navigate","url":"https://example.com/?q=secret"}"#), "navigate");
    assert_eq!(ipc_cmd_shape(r#"{"cmd":"my secret text"}"#), "<non-identifier>");
    assert_eq!(ipc_cmd_shape("secret text"), "<no cmd>");
}

/// The engine's own globals (`__rustkit_*`, `__rk*`) still exist in a web
/// page (its event, timer and history plumbing), but are not enumerable:
/// `Object.keys(window)` lists none of them, as in Chromium.
#[test]
fn a_web_page_enumerates_no_engine_globals_on_window() {
    let (mut engine, id) = page_engine();
    engine.load_html(id, "<p>web</p>").unwrap();
    assert_eq!(
        js(
            &mut engine,
            id,
            "Object.keys(window).filter(function (n) { return n.indexOf('__') === 0; }).join(' ')"
        ),
        ""
    );
    assert_eq!(
        js(
            &mut engine,
            id,
            "Object.getOwnPropertyNames(globalThis).filter(function (n) { return n.indexOf('__rk_') === 0; }).join(' ')"
        ),
        ""
    );
}
