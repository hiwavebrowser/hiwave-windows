//! Module scripts through a real page load (Z package C0, slice 2):
//! `<script type=module>` runs through the module host, its graph is
//! fetched under the page's `FetchPolicy::fetch_module`, and the script
//! element hears `load` or `error`.

use super::*;
use crate::script_net_tests::serve_routes;

type Routes = Vec<(&'static str, &'static str, String)>;

fn load(routes: Routes) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(routes);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

fn log(engine: &mut Engine, view: EngineViewId) -> String {
    let value = engine.execute_script(view, "log.join(',')").unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

const JS: &str = "text/javascript";

fn page(body: &str) -> String {
    format!("<html><head><script>var log = [];</script>{body}</head><body>hi</body></html>")
}

/// The reduced case: a page whose only code is a module. Today the script
/// log says "type=module unsupported" and nothing runs.
#[test]
fn an_external_module_and_its_imports_run() {
    let (mut engine, view, _server) = load(vec![
        ("/", "text/html", page(r#"<script type="module" src="/main.js"></script>"#)),
        ("/main.js", JS, "import { b } from './lib/b.js'; log.push('main:' + b);".into()),
        ("/lib/b.js", JS, "import './c.js'; export const b = 'B'; log.push('b');".into()),
        ("/lib/c.js", JS, "log.push('c');".into()),
    ]);
    assert_eq!(log(&mut engine, view), "c,b,main:B");
    let records = engine.script_log(view).unwrap();
    assert!(
        records.iter().all(|r| !matches!(r.outcome, ScriptOutcome::Skipped(_))),
        "{records:#?}"
    );
}

fn outcomes(engine: &Engine, view: EngineViewId) -> Vec<(String, ScriptOutcome)> {
    engine
        .script_log(view)
        .unwrap()
        .iter()
        .map(|r| (r.source.clone(), r.outcome.clone()))
        .collect()
}

/// A module is deferred: it runs after every classic script, and in document
/// order with the `defer` scripts.
#[test]
fn modules_run_after_classic_scripts_in_order_with_defer_scripts() {
    let (mut engine, view, _server) = load(vec![
        (
            "/",
            "text/html",
            page(
                r#"<script type="module" src="/m1.js"></script>
<script>log.push('classic1');</script>
<script defer src="/d.js"></script>
<script type="module">log.push('inline-module');</script>
<script>log.push('classic2');</script>"#,
            ),
        ),
        ("/m1.js", JS, "log.push('m1');".into()),
        ("/d.js", JS, "log.push('d');".into()),
    ]);
    assert_eq!(log(&mut engine, view), "classic1,classic2,m1,d,inline-module");
}

/// An inline module's imports resolve against the document.
#[test]
fn an_inline_module_imports_relative_to_the_document() {
    let (mut engine, view, _server) = load(vec![
        (
            "/",
            "text/html",
            page(r#"<script type="module">import { x } from './lib/x.js'; log.push('inline:' + x);</script>"#),
        ),
        ("/lib/x.js", JS, "export const x = 'X';".into()),
    ]);
    assert_eq!(log(&mut engine, view), "inline:X");
}

/// The element hears `load` after its module ran, and a module does not see
/// itself as `document.currentScript`.
#[test]
fn the_element_hears_load_and_a_module_has_no_current_script() {
    let (mut engine, view, _server) = load(vec![
        (
            "/",
            "text/html",
            page(
                r#"<script id="m" type="module" src="/m.js"></script>
<script>document.getElementById('m').addEventListener('load', function () { log.push('load'); });</script>"#,
            ),
        ),
        ("/m.js", JS, "log.push('m:' + document.currentScript);".into()),
    ]);
    assert_eq!(log(&mut engine, view), "m:null,load");
}

/// Missing, wrongly typed and refused modules never run and the element
/// hears `error`.
#[test]
fn a_module_that_cannot_be_fetched_fires_error_and_runs_nothing() {
    let other = serve_routes(vec![("/x.js", JS, "window.leaked = true;".into())]);
    let cross = format!("http://127.0.0.1:{}/x.js", other.port);
    let (mut engine, view, _server) = load(vec![
        (
            "/",
            "text/html",
            page(&format!(
                r#"<script id="gone" type="module" src="/missing.js"></script>
<script id="html" type="module" src="/page.js"></script>
<script id="cross" type="module" src="{cross}"></script>
<script>['gone', 'html', 'cross'].forEach(function (id) {{
    document.getElementById(id).addEventListener('error', function () {{ log.push('error:' + id); }});
    document.getElementById(id).addEventListener('load', function () {{ log.push('LOAD:' + id); }});
}});</script>"#
            )),
        ),
        // 200, but not a JavaScript type.
        ("/page.js", "text/html", "log.push('ran');".into()),
    ]);
    assert_eq!(log(&mut engine, view), "error:gone,error:html,error:cross");
    assert_eq!(
        engine.execute_script(view, "typeof window.leaked").unwrap(),
        r#"String("undefined")"#
    );
    let records = outcomes(&engine, view);
    assert_eq!(
        records.iter().filter(|(_, o)| matches!(o, ScriptOutcome::FetchFailed(_))).count(),
        3,
        "{records:#?}"
    );
}

/// A graph that cannot load runs none of itself, however much of it was
/// fine; a syntax error is an `error` too; a throw is `load` plus a record.
#[test]
fn a_broken_graph_fires_error_and_a_throwing_module_still_loads() {
    let (mut engine, view, _server) = load(vec![
        (
            "/",
            "text/html",
            page(
                r#"<script id="dep" type="module" src="/a.js"></script>
<script id="syntax" type="module" src="/syntax.js"></script>
<script id="bare" type="module">import 'react'; log.push('bare ran');</script>
<script id="throws" type="module" src="/throws.js"></script>
<script>['dep', 'syntax', 'bare', 'throws'].forEach(function (id) {
    document.getElementById(id).addEventListener('error', function () { log.push('error:' + id); });
    document.getElementById(id).addEventListener('load', function () { log.push('load:' + id); });
});</script>"#,
            ),
        ),
        ("/a.js", JS, "import './missing-dep.js'; log.push('a ran');".into()),
        ("/syntax.js", JS, "let = = 1;".into()),
        ("/throws.js", JS, "log.push('before'); throw new Error('boom');".into()),
    ]);
    assert_eq!(
        log(&mut engine, view),
        "error:dep,error:syntax,error:bare,before,load:throws"
    );
    let records = outcomes(&engine, view);
    assert!(
        records
            .iter()
            .any(|(_, o)| matches!(o, ScriptOutcome::Threw(m) if m.contains("boom"))),
        "{records:#?}"
    );
}

/// One URL is one module, whichever tags name it.
#[test]
fn a_module_named_twice_runs_once() {
    let (mut engine, view, _server) = load(vec![
        (
            "/",
            "text/html",
            page(
                r#"<script id="a" type="module" src="/once.js"></script>
<script id="b" type="module" src="/once.js"></script>
<script type="module">import './once.js'; log.push('importer');</script>
<script>['a', 'b'].forEach(function (id) {
    document.getElementById(id).addEventListener('load', function () { log.push('load:' + id); });
});</script>"#,
            ),
        ),
        ("/once.js", JS, "log.push('once');".into()),
    ]);
    assert_eq!(log(&mut engine, view), "once,load:a,load:b,importer");
}

#[test]
fn import_meta_url_is_the_served_from_url() {
    let (mut engine, view, server) = load(vec![
        ("/", "text/html", page(r#"<script type="module" src="/lib/u.js"></script>"#)),
        ("/lib/u.js", JS, "log.push(import.meta.url);".into()),
    ]);
    assert_eq!(log(&mut engine, view), format!("http://127.0.0.1:{}/lib/u.js", server.port));
}
