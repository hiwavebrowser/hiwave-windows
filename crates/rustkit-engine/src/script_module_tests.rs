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

/// S12 (a store page names one component module from every instance:
/// 220 module tags, 48 URLs). The module map keys the fetch, not just the
/// evaluation: Chromium 143 sends one request for five tags naming one URL.
/// Fetching each tag's copy queued 189 duplicate fetches behind the
/// 8-wide script fetch window, holding the first script back by 11-17 s.
#[test]
fn a_module_named_by_many_tags_is_fetched_once() {
    let tags = r#"<script type="module" src="/once.js"></script>"#.repeat(5);
    let (mut engine, view, server) = load(vec![
        ("/", "text/html", page(&tags)),
        ("/once.js", JS, "log.push('once');".into()),
    ]);
    assert_eq!(log(&mut engine, view), "once");
    // The document, then the module once.
    assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn import_meta_url_is_the_served_from_url() {
    let (mut engine, view, server) = load(vec![
        ("/", "text/html", page(r#"<script type="module" src="/lib/u.js"></script>"#)),
        ("/lib/u.js", JS, "log.push(import.meta.url);".into()),
    ]);
    assert_eq!(log(&mut engine, view), format!("http://127.0.0.1:{}/lib/u.js", server.port));
}

/// The github pattern: an element in the markup, a module that defines it.
/// Before custom elements, `customElements is not defined` stopped the module
/// (and with it every script after it).
#[test]
fn a_module_defines_a_custom_element_that_upgrades_the_markup() {
    let (mut engine, view, _server) = load(vec![
        (
            "/",
            "text/html",
            page(
                r#"<my-card id="c" title="Hello"></my-card>
<script type="module" src="/card.js"></script>
<script>document.body.appendChild(Object.assign(document.createElement('my-card'), { id: 'made' }));</script>"#,
            ),
        ),
        (
            "/card.js",
            JS,
            "class MyCard extends HTMLElement { \
                 static get observedAttributes() { return ['title']; } \
                 connectedCallback() { this.textContent = 'card:' + (this.getAttribute('title') || 'none'); log.push('connected:' + this.id); } \
                 attributeChangedCallback(n, o, v) { log.push('attr:' + n + ':' + v); } \
             } \
             customElements.define('my-card', MyCard); \
             log.push('defined:' + (customElements.get('my-card') === MyCard));"
                .into(),
        ),
    ]);
    assert_eq!(log(&mut engine, view), "attr:title:Hello,connected:c,connected:made,defined:true");
    // The upgraded element's own work shows in the document.
    assert_eq!(
        engine.execute_script(view, "document.getElementById('c').textContent").unwrap(),
        r#"String("card:Hello")"#
    );
    // `define` upgrades synchronously (so the callbacks come before the line
    // after it), including the element the classic script made and connected
    // before the module ran.
    assert_eq!(
        engine.execute_script(view, "document.getElementById('made') instanceof customElements.get('my-card')").unwrap(),
        "Boolean(true)"
    );
}

/// The github pattern: bare specifiers named by the page's import map.
#[test]
fn an_import_map_names_the_bare_specifiers_a_module_imports() {
    let (mut engine, view, _server) = load(vec![
        (
            "/",
            "text/html",
            page(
                r#"<script type="importmap">{ "imports": { "lib": "/vendor/lib.js", "util/": "/vendor/util/" } }</script>
<script type="module">import { v } from 'lib'; import { u } from 'util/x.js'; log.push('mapped:' + v + u);</script>
<script type="module">import 'not-in-the-map'; log.push('SHOULD NOT RUN');</script>"#,
            ),
        ),
        ("/vendor/lib.js", JS, "export const v = 'V';".into()),
        ("/vendor/util/x.js", JS, "export const u = 'U';".into()),
    ]);
    assert_eq!(log(&mut engine, view), "mapped:VU");
    let records = outcomes(&engine, view);
    assert!(
        records
            .iter()
            .any(|(_, o)| matches!(o, ScriptOutcome::Threw(m) if m.contains("not-in-the-map"))),
        "{records:#?}"
    );
}

/// An import map that cannot be used is reported and changes nothing.
#[test]
fn a_broken_import_map_is_reported_and_the_page_carries_on() {
    let (mut engine, view, _server) = load(vec![
        (
            "/",
            "text/html",
            page(r#"<script type="importmap">{ not json</script><script>log.push('classic still runs');</script>"#),
        ),
    ]);
    assert_eq!(log(&mut engine, view), "classic still runs");
    let records = outcomes(&engine, view);
    assert!(
        records
            .iter()
            .any(|(src, o)| src == "importmap" && matches!(o, ScriptOutcome::EngineFailed(EngineFailure::ImportMap(_)))),
        "{records:#?}"
    );
}

/// import() from a classic script, from a timer, and from a module; one that
/// cannot be fetched rejects instead of hanging the load.
#[test]
fn dynamic_import_loads_modules_for_classic_scripts_timers_and_modules() {
    let (mut engine, view, _server) = load(vec![
        (
            "/",
            "text/html",
            page(
                r#"<script>
import('/lazy/a.js').then(function (m) { log.push('classic:' + m.name); });
setTimeout(function () { import('/lazy/b.js').then(function (m) { log.push('timer:' + m.name); }); }, 50);
import('/missing/gone.js').then(function () { log.push('RESOLVED'); }, function (e) { log.push('rejected:' + e.name); });
import('bare-specifier').then(function () { log.push('RESOLVED'); }, function (e) { log.push('bare:' + e.name); });
</script>
<script type="module">const m = await import('/lazy/c.js'); log.push('module:' + m.name);</script>"#,
            ),
        ),
        ("/lazy/a.js", JS, "export const name = 'A';".into()),
        ("/lazy/b.js", JS, "export const name = 'B';".into()),
        ("/lazy/c.js", JS, "export const name = 'C';".into()),
    ]);
    let got = log(&mut engine, view);
    let mut parts: Vec<&str> = got.split(',').collect();
    parts.sort();
    assert_eq!(
        parts,
        vec!["bare:TypeError", "classic:A", "module:C", "rejected:TypeError", "timer:B"],
        "{got}"
    );
}

// S15 (microsoft.com: 87 static imports in 43 waves, ~8 s of idle network
// inside the script task, `load` at 55.7 s). A module's static import graph
// was walked only when that module's turn to run came, one level at a time.
// Chromium fetches the whole static graph as soon as the module script is
// discovered, then evaluates in order. Each page below also carries a
// classic script the server holds for 300 ms (`/held...`): the page's
// scripts are still arriving while it is out, and nothing has run yet.

/// The request log's one entry for `path`.
fn served_once(server: &crate::script_net_tests::Server, path: &str) -> crate::script_net_tests::ServedRequest {
    let hits = server.served(path);
    assert_eq!(hits.len(), 1, "{path} requested {} times: {:#?}", hits.len(), server.requests.lock().unwrap());
    hits.into_iter().next().unwrap()
}

/// A three-deep chain: every module of it is requested while the held
/// classic script is still out, so before any script ran and before `a`
/// evaluates; each once; and evaluation order is c, b, a after the classic.
#[test]
fn a_module_graph_is_fetched_on_discovery_not_when_it_runs() {
    let (mut engine, view, server) = load(vec![
        (
            "/",
            "text/html",
            page(r#"<script type="module" src="/a.js"></script><script src="/held-classic.js"></script>"#),
        ),
        ("/held-classic.js", JS, "log.push('classic');".into()),
        ("/a.js", JS, "import { b } from './lib/b.js'; log.push('a:' + b);".into()),
        ("/lib/b.js", JS, "import { c } from './c.js';\nexport const b = 'B' + c; log.push('b');".into()),
        ("/lib/c.js", JS, "export const c = 'C'; log.push('c');".into()),
    ]);
    assert_eq!(log(&mut engine, view), "classic,c,b,a:BC");
    let held = served_once(&server, "/held-classic.js");
    let held_answered = held.answered.expect("held script answered");
    for path in ["/a.js", "/lib/b.js", "/lib/c.js"] {
        let hit = served_once(&server, path);
        assert!(
            hit.arrived < held_answered,
            "{path} was requested {:?} after the page's held script was answered: the graph waited for its turn to run",
            hit.arrived - held_answered
        );
    }
}

/// `import()` stays lazy, and an `import` in a comment, a string or a
/// template is not an import.
#[test]
fn a_prefetched_graph_leaves_dynamic_imports_lazy() {
    let (mut engine, view, server) = load(vec![
        (
            "/",
            "text/html",
            page(r#"<script type="module" src="/a.js"></script><script src="/held-classic.js"></script>"#),
        ),
        ("/held-classic.js", JS, "log.push('classic');".into()),
        (
            "/a.js",
            JS,
            r#"import './b.js';
// import './in-comment.js';
/* import x from './in-block-comment.js'; */
const s = "import './in-string.js'";
const t = `export * from './in-template.js'`;
window.later = function () { return import('./lazy.js'); };
log.push('a');"#
                .into(),
        ),
        ("/b.js", JS, "log.push('b');".into()),
        ("/lazy.js", JS, "log.push('lazy');".into()),
    ]);
    assert_eq!(log(&mut engine, view), "classic,b,a");
    let held_answered = served_once(&server, "/held-classic.js").answered.unwrap();
    assert!(served_once(&server, "/b.js").arrived < held_answered);
    for path in ["/lazy.js", "/in-comment.js", "/in-block-comment.js", "/in-string.js", "/in-template.js"] {
        assert!(server.served(path).is_empty(), "{path} was fetched");
    }
}

/// A cycle is walked once and does not hang; each module is requested once.
#[test]
fn a_cyclic_graph_is_prefetched_once_and_runs() {
    let (mut engine, view, server) = load(vec![
        (
            "/",
            "text/html",
            page(r#"<script type="module" src="/a.js"></script><script src="/held-classic.js"></script>"#),
        ),
        ("/held-classic.js", JS, "log.push('classic');".into()),
        ("/a.js", JS, "import { b } from './b.js'; export const a = 'A'; log.push('a:' + b);".into()),
        ("/b.js", JS, "import { a } from './a.js'; export const b = 'B'; log.push('b');".into()),
    ]);
    assert_eq!(log(&mut engine, view), "classic,b,a:B");
    let held_answered = served_once(&server, "/held-classic.js").answered.unwrap();
    assert!(served_once(&server, "/a.js").arrived < held_answered);
    assert!(served_once(&server, "/b.js").arrived < held_answered);
}

/// A 404 inside one graph fails that module's evaluation only, logged as
/// before ("module graph failed"), and its element hears `error`; the other
/// module's graph runs.
#[test]
fn a_missing_module_in_a_prefetched_graph_fails_only_its_own_module() {
    let (mut engine, view, server) = load(vec![
        (
            "/",
            "text/html",
            page(
                r#"<script id="broken" type="module" src="/m1.js"></script>
<script id="fine" type="module" src="/m2.js"></script>
<script src="/held-classic.js"></script>
<script>['broken', 'fine'].forEach(function (id) {
    document.getElementById(id).addEventListener('error', function () { log.push('error:' + id); });
    document.getElementById(id).addEventListener('load', function () { log.push('load:' + id); });
});</script>"#,
            ),
        ),
        ("/held-classic.js", JS, "log.push('classic');".into()),
        ("/m1.js", JS, "import './lib/x.js'; log.push('m1 ran');".into()),
        ("/lib/x.js", JS, "import '/missing-dep.js'; log.push('x ran');".into()),
        ("/m2.js", JS, "import './lib/ok.js'; log.push('m2');".into()),
        ("/lib/ok.js", JS, "log.push('ok');".into()),
    ]);
    assert_eq!(log(&mut engine, view), "classic,error:broken,ok,m2,load:fine");
    let held_answered = served_once(&server, "/held-classic.js").answered.unwrap();
    assert!(served_once(&server, "/missing-dep.js").arrived < held_answered);
    served_once(&server, "/lib/ok.js");
    let records = outcomes(&engine, view);
    assert!(
        records
            .iter()
            .any(|(src, o)| src.ends_with("/m1.js")
                && matches!(o, ScriptOutcome::Threw(m) if m.starts_with("module graph failed"))),
        "{records:#?}"
    );
}

/// Review B2 on S15: a slow import must not hold the page's classic
/// scripts. The prefetch walk stops when the page's scripts are in; a fetch
/// still out is awaited only by the module that imports it.
#[test]
fn a_slow_prefetched_import_does_not_hold_the_classic_scripts() {
    let (mut engine, view, server) = load(vec![
        (
            "/",
            "text/html",
            page(r#"<script type="module" src="/a.js"></script><script>log.push('classic'); window.classicAt = Date.now();</script>"#),
        ),
        ("/a.js", JS, "import './dep.js?delay=3000'; log.push('a');".into()),
        ("/dep.js?delay=3000", JS, "log.push('dep');".into()),
    ]);
    assert_eq!(log(&mut engine, view), "classic,dep,a");
    let classic_at = engine.execute_script(view, "String(window.classicAt)").unwrap();
    let classic_ms: u128 = classic_at
        .trim_start_matches("String(\"")
        .trim_end_matches("\")")
        .parse()
        .unwrap_or_else(|_| panic!("classicAt: {classic_at}"));
    let dep = served_once(&server, "/dep.js?delay=3000");
    let dep_ms = dep
        .answered_wall
        .expect("dep answered")
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    assert!(
        classic_ms < dep_ms,
        "the classic script ran {} ms after the slow import was answered: it waited for the module graph",
        classic_ms - dep_ms
    );
}

/// R1 B1: a module whose text ends inside a string on a backslash (a body
/// cut off at the network cap, say) is network input the prefetch lexer
/// reads unchecked. It must not panic the load: the module is a syntax
/// error for its own element (logged, `error`), and the page's other
/// scripts run.
#[test]
fn a_module_cut_off_inside_a_string_is_a_module_error_not_a_panic() {
    let (mut engine, view, _server) = load(vec![
        (
            "/",
            "text/html",
            page(
                r#"<script id="cut" type="module" src="/cut.js"></script>
<script>document.getElementById('cut').addEventListener('error', function () { log.push('error:cut'); });
log.push('classic');</script>"#,
            ),
        ),
        ("/cut.js", JS, "log.push('cut ran'); import \"a\\".into()),
    ]);
    assert_eq!(log(&mut engine, view), "classic,error:cut");
    let records = outcomes(&engine, view);
    assert!(
        records
            .iter()
            .any(|(src, o)| src.ends_with("/cut.js") && matches!(o, ScriptOutcome::Threw(_))),
        "{records:#?}"
    );
}

/// Batch 19 HOLD on S15 (github.com stalled after a module script, 6 of 8
/// captures). The prefetch walk stops once the page's scripts are in, but
/// the fetches it started kept their place in the policy's per-origin queue
/// (six at a time, first come first served) with nothing polling them: an
/// answered prefetch never gave its slot back. A fetch the walk did not
/// start (`late.js`, imported by a module that arrived after the walk
/// stopped) then waited for a slot until its network cap. Here one graph
/// shares a slow import with another and the second graph alone fills the
/// origin's six slots.
#[test]
fn prefetches_left_running_never_starve_a_later_module_fetch() {
    let (mut engine, view, server) = load(vec![
        (
            "/",
            "text/html",
            page(
                r#"<script type="module" src="/m1.js"></script>
<script type="module" src="/m2.js"></script>
<script src="/held-classic.js"></script>"#,
            ),
        ),
        // Held 300 ms: the walk is still going when every prefetch has been
        // polled into the origin's queue (six slots, eight fetches), and has
        // stopped before their 400 ms answers.
        ("/held-classic.js", JS, "log.push('classic');".into()),
        ("/m1.js", JS, "import './a.js?delay=400'; import './shared.js?delay=400'; log.push('m1');".into()),
        ("/a.js?delay=400", JS, "import './late.js'; log.push('a');".into()),
        ("/late.js", JS, "log.push('late');".into()),
        ("/shared.js?delay=400", JS, "log.push('shared');".into()),
        (
            "/m2.js",
            JS,
            "import './shared.js?delay=400'; import './b1.js?delay=400'; import './b2.js?delay=400'; \
             import './b3.js?delay=400'; import './b4.js?delay=400'; import './b5.js?delay=400'; \
             import './b6.js?delay=400'; log.push('m2');"
                .into(),
        ),
        ("/b1.js?delay=400", JS, "log.push('b1');".into()),
        ("/b2.js?delay=400", JS, "log.push('b2');".into()),
        ("/b3.js?delay=400", JS, "log.push('b3');".into()),
        ("/b4.js?delay=400", JS, "log.push('b4');".into()),
        ("/b5.js?delay=400", JS, "log.push('b5');".into()),
        ("/b6.js?delay=400", JS, "log.push('b6');".into()),
    ]);
    assert_eq!(log(&mut engine, view), "classic,late,a,shared,m1,b1,b2,b3,b4,b5,b6,m2");
    // Timed on the server, from the document's request to `late.js`'s
    // answer (the engine's own start can wait on the GPU test guard).
    let document = served_once(&server, "/");
    let late = served_once(&server, "/late.js").answered.expect("late.js answered");
    let elapsed = late - document.arrived;
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "late.js was answered {elapsed:?} after the page was requested: it waited on prefetch slots nobody polled"
    );
}
