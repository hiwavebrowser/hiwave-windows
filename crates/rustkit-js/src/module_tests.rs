//! The module host (module.rs). The "network" here is a map of URL to
//! source; what is under test is the graph, the identity rules and the
//! failure modes, none of which need a socket.

use crate::{FetchedModule, JsRuntime, JsValue, ModuleHandle, ModuleState};
use std::collections::HashMap;

struct Site {
    files: HashMap<&'static str, &'static str>,
    /// URL -> the URL it is "served from" after a redirect.
    redirects: HashMap<&'static str, &'static str>,
    asked: Vec<String>,
}

impl Site {
    fn new(files: &[(&'static str, &'static str)]) -> Self {
        Self {
            files: files.iter().cloned().collect(),
            redirects: HashMap::new(),
            asked: Vec::new(),
        }
    }

    /// Fetch what the graph has asked for until it asks for nothing more.
    fn serve(&mut self, rt: &mut JsRuntime) {
        loop {
            let requests = rt.take_module_requests();
            if requests.is_empty() {
                return;
            }
            for url in requests {
                self.asked.push(url.clone());
                let served = self.redirects.get(url.as_str()).copied().unwrap_or(url.as_str());
                let outcome = match self.files.get(served) {
                    Some(source) => Ok(FetchedModule {
                        final_url: served.to_string(),
                        source: source.to_string(),
                    }),
                    None => Err(format!("404 for {url}")),
                };
                rt.supply_module(&url, outcome);
            }
        }
    }
}

fn runtime() -> JsRuntime {
    let mut rt = JsRuntime::new().unwrap();
    rt.set_module_base("https://site.test/page/index.html");
    rt.evaluate_script("var log = [];").unwrap();
    rt
}

fn run(rt: &mut JsRuntime, site: &mut Site, url: &str) -> (ModuleHandle, ModuleState) {
    let source = site.files.get(url).copied().unwrap_or_else(|| panic!("no root {url}"));
    let handle = rt.begin_module(url, source).expect("parses");
    site.serve(rt);
    let state = rt.poll_module(&handle);
    (handle, state)
}

fn log(rt: &mut JsRuntime) -> String {
    match rt.evaluate_script("log.join(',')").unwrap() {
        JsValue::String(s) => s,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_module_with_no_imports_runs() {
    let mut rt = runtime();
    let mut site = Site::new(&[("https://site.test/a.js", "log.push('a'); export const x = 1;")]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert_eq!(state, ModuleState::Done);
    assert_eq!(log(&mut rt), "a");
    assert!(site.asked.is_empty(), "no import, nothing asked");
}

/// Dependencies run first, in import order, and a module two others import
/// runs once.
#[test]
fn a_graph_evaluates_dependencies_first_and_each_module_once() {
    let mut rt = runtime();
    let mut site = Site::new(&[
        ("https://site.test/a.js", "import './b.js'; import './c.js'; log.push('a');"),
        ("https://site.test/b.js", "import './d.js'; log.push('b');"),
        ("https://site.test/c.js", "import './d.js'; log.push('c');"),
        ("https://site.test/d.js", "log.push('d');"),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert_eq!(state, ModuleState::Done);
    assert_eq!(log(&mut rt), "d,b,c,a");
    assert_eq!(site.asked.len(), 3, "d was asked for once: {:?}", site.asked);
}

/// A specifier resolves against the importing module's URL, not the page's.
#[test]
fn specifiers_resolve_against_the_importing_module() {
    let mut rt = runtime();
    let mut site = Site::new(&[
        ("https://site.test/app/main.js", "import '../lib/util.js'; import '/root.js'; import 'https://cdn.test/x.js'; log.push('main');"),
        ("https://site.test/lib/util.js", "import './deep/inner.js'; log.push('util');"),
        ("https://site.test/lib/deep/inner.js", "log.push('inner');"),
        ("https://site.test/root.js", "log.push('root');"),
        ("https://cdn.test/x.js", "log.push('x');"),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/app/main.js");
    assert_eq!(state, ModuleState::Done);
    assert_eq!(log(&mut rt), "inner,util,root,x,main");
    assert_eq!(
        site.asked,
        vec![
            "https://site.test/lib/util.js",
            "https://site.test/root.js",
            "https://cdn.test/x.js",
            "https://site.test/lib/deep/inner.js",
        ]
    );
}

#[test]
fn a_bare_specifier_fails_without_asking_for_anything() {
    let mut rt = runtime();
    let mut site = Site::new(&[("https://site.test/a.js", "import 'react'; log.push('a');")]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    match state {
        ModuleState::Failed(message) => assert!(message.contains("react"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert!(site.asked.is_empty());
    assert_eq!(log(&mut rt), "", "no module code ran");
}

/// A missing dependency, a malformed one and a link error each fail the
/// whole graph before any of its code runs.
#[test]
fn a_graph_that_cannot_be_loaded_runs_none_of_it() {
    // Missing.
    let mut rt = runtime();
    let mut site = Site::new(&[("https://site.test/a.js", "import './gone.js'; log.push('a');")]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert!(matches!(state, ModuleState::Failed(_)), "{state:?}");
    assert_eq!(log(&mut rt), "");

    // A syntax error in a dependency.
    let mut rt = runtime();
    let mut site = Site::new(&[
        ("https://site.test/a.js", "import './bad.js'; log.push('a');"),
        ("https://site.test/bad.js", "export const = ;"),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert!(matches!(state, ModuleState::Failed(_)), "{state:?}");
    assert_eq!(log(&mut rt), "");

    // An import the dependency does not export.
    let mut rt = runtime();
    let mut site = Site::new(&[
        ("https://site.test/a.js", "import { nope } from './b.js'; log.push('a');"),
        ("https://site.test/b.js", "log.push('b'); export const yes = 1;"),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert!(matches!(state, ModuleState::Failed(_)), "{state:?}");
    assert_eq!(log(&mut rt), "", "linking failed before b ran");
}

#[test]
fn a_syntax_error_in_the_root_is_reported_before_anything_is_asked() {
    let mut rt = runtime();
    let error = rt.begin_module("https://site.test/a.js", "import './b.js'; let = = 1;").unwrap_err();
    assert!(!error.is_empty());
    assert!(rt.take_module_requests().is_empty());
}

#[test]
fn an_exception_while_evaluating_is_threw_not_failed() {
    let mut rt = runtime();
    let mut site = Site::new(&[
        ("https://site.test/a.js", "import './b.js'; log.push('a');"),
        ("https://site.test/b.js", "log.push('b'); throw new Error('boom');"),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    match state {
        ModuleState::Threw(message) => assert!(message.contains("boom"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(log(&mut rt), "b", "b ran, a did not");
}

/// Two scripts naming one module share it. And a redirect that lands on a
/// URL already loaded does not run it again.
#[test]
fn one_url_is_one_module_even_through_a_redirect() {
    let mut rt = runtime();
    let mut site = Site::new(&[
        ("https://site.test/a.js", "import './lib.js'; log.push('a');"),
        ("https://site.test/b.js", "import './old-lib.js'; log.push('b');"),
        ("https://site.test/lib.js", "log.push('lib');"),
    ]);
    // old-lib.js is served from lib.js.
    site.redirects.insert("https://site.test/old-lib.js", "https://site.test/lib.js");

    let (_, a) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert_eq!(a, ModuleState::Done);
    let (_, b) = run(&mut rt, &mut site, "https://site.test/b.js");
    assert_eq!(b, ModuleState::Done);
    assert_eq!(log(&mut rt), "lib,a,b", "lib evaluated once");

    // Starting the same URL again is the same module, not a second run.
    let again = rt.begin_module("https://site.test/a.js", "log.push('SECOND');").unwrap();
    assert_eq!(rt.poll_module(&again), ModuleState::Done);
    assert_eq!(log(&mut rt), "lib,a,b");
}

#[test]
fn a_cycle_loads_and_runs() {
    let mut rt = runtime();
    let mut site = Site::new(&[
        ("https://site.test/a.js", "import { b } from './b.js'; export const a = 'A'; log.push('a:' + b);"),
        ("https://site.test/b.js", "import './a.js'; export const b = 'B'; log.push('b');"),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert_eq!(state, ModuleState::Done);
    assert_eq!(log(&mut rt), "b,a:B");
}

/// A top-level `await` that needs a later job: the module is Evaluating,
/// then Done once the job has run.
#[test]
fn top_level_await_finishes_when_its_promise_does() {
    let mut rt = runtime();
    let mut site = Site::new(&[(
        "https://site.test/a.js",
        "log.push('before'); await Promise.resolve(); log.push('after');",
    )]);
    let (handle, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert!(matches!(state, ModuleState::Done | ModuleState::Evaluating), "{state:?}");
    assert_eq!(rt.poll_module(&handle), ModuleState::Done);
    assert_eq!(log(&mut rt), "before,after");
}

#[test]
fn import_meta_url_is_where_the_module_was_served_from() {
    let mut rt = runtime();
    let mut site = Site::new(&[
        ("https://site.test/dir/a.js", "import './b.js'; log.push(import.meta.url);"),
        ("https://site.test/dir/b.js", "log.push(import.meta.url);"),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/dir/a.js");
    assert_eq!(state, ModuleState::Done);
    assert_eq!(log(&mut rt), "https://site.test/dir/b.js,https://site.test/dir/a.js");
}

/// Module code sees the page's globals and a module's own bindings stay out
/// of the global scope.
#[test]
fn module_scope_is_not_global_scope() {
    let mut rt = runtime();
    rt.evaluate_script("var fromPage = 'page';").unwrap();
    let mut site = Site::new(&[(
        "https://site.test/a.js",
        "var local = 1; log.push(typeof fromPage + ':' + typeof local + ':' + typeof this);",
    )]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert_eq!(state, ModuleState::Done);
    assert_eq!(log(&mut rt), "string:number:undefined");
    match rt.evaluate_script("typeof local").unwrap() {
        JsValue::String(s) => assert_eq!(s, "undefined"),
        other => panic!("{other:?}"),
    }
}

/// The host is never asked for the same URL twice, however many modules
/// import it.
#[test]
fn a_url_is_asked_for_once() {
    let mut rt = runtime();
    let mut site = Site::new(&[
        ("https://site.test/a.js", "import './x.js'; import './y.js'; import './x.js';"),
        ("https://site.test/x.js", "import './y.js';"),
        ("https://site.test/y.js", "log.push('y');"),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert_eq!(state, ModuleState::Done);
    assert_eq!(site.asked.len(), 2, "{:?}", site.asked);
}

// ---- import maps and dynamic import()

fn runtime_with_map(map: &str) -> JsRuntime {
    let mut rt = runtime();
    let warnings = rt.add_import_map(map).expect("a usable import map");
    assert!(warnings.is_empty(), "{warnings:?}");
    rt
}

/// The github case: a module imports a bare specifier the page's import map
/// names.
#[test]
fn a_bare_specifier_resolves_through_the_import_map() {
    let mut rt = runtime_with_map(r#"{ "imports": { "react": "https://cdn.test/react.js", "lib": "/vendor/lib.js" } }"#);
    let mut site = Site::new(&[
        ("https://site.test/a.js", "import 'react'; import 'lib'; log.push('a');"),
        ("https://cdn.test/react.js", "log.push('react');"),
        ("https://site.test/vendor/lib.js", "log.push('lib');"),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert_eq!(state, ModuleState::Done);
    assert_eq!(log(&mut rt), "react,lib,a");
    assert_eq!(site.asked, vec!["https://cdn.test/react.js", "https://site.test/vendor/lib.js"]);
}

#[test]
fn the_longest_matching_prefix_wins_and_the_rest_is_appended() {
    let mut rt = runtime_with_map(
        r#"{ "imports": { "util/": "/u/", "util/deep/": "https://cdn.test/d/", "exact": "/exact.js", "exact/": "/exact-dir/" } }"#,
    );
    let mut site = Site::new(&[
        ("https://site.test/a.js", "import 'util/x.js'; import 'util/deep/y.js'; import 'exact'; import 'exact/z.js';"),
        ("https://site.test/u/x.js", ""),
        ("https://cdn.test/d/y.js", ""),
        ("https://site.test/exact.js", ""),
        ("https://site.test/exact-dir/z.js", ""),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    assert_eq!(state, ModuleState::Done);
    let mut asked = site.asked.clone();
    asked.sort();
    assert_eq!(
        asked,
        vec![
            "https://cdn.test/d/y.js",
            "https://site.test/exact-dir/z.js",
            "https://site.test/exact.js",
            "https://site.test/u/x.js",
        ]
    );
}

/// A scope applies to the modules under its prefix, before the top-level map.
#[test]
fn scopes_apply_to_their_referrers_first() {
    let mut rt = runtime_with_map(
        r#"{ "imports": { "dep": "/top/dep.js" }, "scopes": { "/app/": { "dep": "/scoped/dep.js" }, "/app/inner/": { "dep": "/inner/dep.js" } } }"#,
    );
    let mut site = Site::new(&[
        ("https://site.test/root.js", "import 'dep'; import './app/mid.js'; import './app/inner/deep.js';"),
        ("https://site.test/app/mid.js", "import 'dep';"),
        ("https://site.test/app/inner/deep.js", "import 'dep';"),
        ("https://site.test/top/dep.js", ""),
        ("https://site.test/scoped/dep.js", ""),
        ("https://site.test/inner/dep.js", ""),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/root.js");
    assert_eq!(state, ModuleState::Done);
    for expected in ["https://site.test/top/dep.js", "https://site.test/scoped/dep.js", "https://site.test/inner/dep.js"] {
        assert!(site.asked.iter().any(|u| u == expected), "{expected} not asked: {:?}", site.asked);
    }
}

#[test]
fn a_null_address_blocks_the_specifier_and_asks_for_nothing() {
    let mut rt = runtime_with_map(r#"{ "imports": { "blocked": null, "blocked-dir/": null } }"#);
    let mut site = Site::new(&[("https://site.test/a.js", "import 'blocked'; log.push('a');")]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/a.js");
    match state {
        ModuleState::Failed(m) => assert!(m.contains("blocked"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(site.asked.is_empty());
    let mut site = Site::new(&[("https://site.test/b.js", "import 'blocked-dir/x.js';")]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/b.js");
    assert!(matches!(state, ModuleState::Failed(_)), "{state:?}");
}

/// A relative or absolute specifier can be remapped too: keys are normalised
/// to URLs.
#[test]
fn a_url_specifier_can_be_remapped() {
    let mut rt = runtime_with_map(
        r#"{ "imports": { "./old.js": "./new.js", "https://cdn.test/v1/x.js": "https://cdn.test/v2/x.js" } }"#,
    );
    let mut site = Site::new(&[
        ("https://site.test/page/a.js", "import './old.js'; import 'https://cdn.test/v1/x.js';"),
        ("https://site.test/page/new.js", ""),
        ("https://cdn.test/v2/x.js", ""),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/page/a.js");
    assert_eq!(state, ModuleState::Done);
    assert_eq!(site.asked, vec!["https://site.test/page/new.js", "https://cdn.test/v2/x.js"]);
}

#[test]
fn bad_entries_are_dropped_with_a_warning_and_a_bad_document_changes_nothing() {
    let mut rt = runtime();
    let warnings = rt
        .add_import_map(r#"{ "imports": { "good": "/g.js", "bare-address": "not-a-url", "dir/": "/no-slash", "num": 5, "": "/x.js" } }"#)
        .unwrap();
    assert_eq!(warnings.len(), 4, "{warnings:?}");
    assert!(rt.add_import_map("{ not json").is_err());
    assert!(rt.add_import_map("[]").is_err());
    assert!(rt.add_import_map(r#"{ "imports": [] }"#).is_err());
    assert!(rt.add_import_map(r#"{ "scopes": { "/a/": 3 } }"#).is_err());
    let mut site = Site::new(&[
        ("https://site.test/a.js", "import 'good';"),
        ("https://site.test/g.js", ""),
        ("https://site.test/b.js", "import 'bare-address';"),
        ("https://site.test/c.js", "import 'dir/x';"),
    ]);
    assert_eq!(run(&mut rt, &mut site, "https://site.test/a.js").1, ModuleState::Done);
    for url in ["https://site.test/b.js", "https://site.test/c.js"] {
        assert!(matches!(run(&mut rt, &mut site, url).1, ModuleState::Failed(_)), "{url}");
    }
}

#[test]
fn a_later_map_adds_keys_and_never_overrides_an_earlier_one() {
    let mut rt = runtime_with_map(r#"{ "imports": { "a": "/first/a.js" } }"#);
    rt.add_import_map(r#"{ "imports": { "a": "/second/a.js", "b": "/second/b.js" } }"#).unwrap();
    let mut site = Site::new(&[
        ("https://site.test/m.js", "import 'a'; import 'b';"),
        ("https://site.test/first/a.js", ""),
        ("https://site.test/second/b.js", ""),
    ]);
    assert_eq!(run(&mut rt, &mut site, "https://site.test/m.js").1, ModuleState::Done);
    assert_eq!(site.asked, vec!["https://site.test/first/a.js", "https://site.test/second/b.js"]);
}

/// import() from a classic script: the host is asked, then the promise
/// resolves with the namespace (or rejects when the host has no answer).
#[test]
fn dynamic_import_is_recorded_then_resolves_or_rejects() {
    let mut rt = runtime();
    rt.evaluate_script(
        "import('./dyn.js').then(function (m) { log.push('dyn:' + m.value + ':' + m.default); }, function (e) { log.push('rej:' + e.name); }); \
         import('./gone.js').then(function () { log.push('GONE-RESOLVED'); }, function (e) { log.push('gone:' + e.name); }); \
         import('bare').then(function () {}, function (e) { log.push('bare:' + e.name); });",
    )
    .unwrap();
    let mut site = Site::new(&[("https://site.test/page/dyn.js", "export const value = 7; export default 'd'; log.push('dyn ran');")]);
    site.serve(&mut rt);
    assert_eq!(site.asked, vec!["https://site.test/page/dyn.js", "https://site.test/page/gone.js"], "relative to the document");
    assert_eq!(log(&mut rt), "bare:TypeError,dyn ran,dyn:7:d,gone:TypeError");
}

/// A dynamic import from inside a module resolves against that module.
#[test]
fn a_dynamic_import_inside_a_module_resolves_against_it() {
    let mut rt = runtime();
    let mut site = Site::new(&[
        ("https://site.test/app/main.js", "import('./lazy.js').then(function (m) { log.push('lazy:' + m.x); });"),
        ("https://site.test/app/lazy.js", "export const x = 'L';"),
    ]);
    let (_, state) = run(&mut rt, &mut site, "https://site.test/app/main.js");
    assert_eq!(state, ModuleState::Done);
    site.serve(&mut rt);
    assert_eq!(site.asked, vec!["https://site.test/app/lazy.js"]);
    assert_eq!(log(&mut rt), "lazy:L");
}

/// A dynamic import that stays pending across multiple intermediate turns / run_jobs calls
/// resumes cleanly and resolves once the host supplies the module.
#[test]
fn dynamic_import_pending_across_multiple_run_jobs_resolves() {
    let mut rt = runtime();
    // 1. Initiate dynamic import - enters pending state waiting for network fetch
    rt.evaluate_script(
        "import('./delayed.js').then(function (m) { log.push('resolved:' + m.val); });",
    )
    .unwrap();
    assert_eq!(
        rt.take_module_requests(),
        vec!["https://site.test/page/delayed.js"]
    );
    assert_eq!(log(&mut rt), ""); // Not resolved yet

    // 2. Multiple intermediate script evaluations and run_jobs calls while fetch is in-flight
    rt.evaluate_script("log.push('intermediate turn 1');").unwrap();
    rt.evaluate_script("log.push('intermediate turn 2');").unwrap();
    assert_eq!(log(&mut rt), "intermediate turn 1,intermediate turn 2");

    // 3. Now supply the fetched module and let jobs run to resolve
    rt.supply_module(
        "https://site.test/page/delayed.js",
        Ok(FetchedModule {
            final_url: "https://site.test/page/delayed.js".into(),
            source: "export const val = 42;".into(),
        }),
    );
    assert_eq!(
        log(&mut rt),
        "intermediate turn 1,intermediate turn 2,resolved:42"
    );
}

/// Two runtimes running simultaneously must own distinct executor contexts without
/// global placeholder aliasing or cross-talk, even with concurrent pending imports.
#[test]
fn two_runtimes_execute_concurrently_without_aliasing() {
    let mut rt1 = runtime();
    let mut rt2 = runtime();

    // 1. Both runtimes evaluate code and initiate separate pending dynamic imports
    rt1.evaluate_script(
        "import('./mod1.js').then(function (m) { log.push('rt1:' + m.val); });",
    )
    .unwrap();
    rt2.evaluate_script(
        "import('./mod2.js').then(function (m) { log.push('rt2:' + m.val); });",
    )
    .unwrap();

    assert_eq!(rt1.take_module_requests(), vec!["https://site.test/page/mod1.js"]);
    assert_eq!(rt2.take_module_requests(), vec!["https://site.test/page/mod2.js"]);

    // 2. Interleaved evaluation across both runtimes
    rt1.evaluate_script("log.push('turn1');").unwrap();
    rt2.evaluate_script("log.push('turn2');").unwrap();

    // 3. Resolve rt2 first
    rt2.supply_module(
        "https://site.test/page/mod2.js",
        Ok(FetchedModule {
            final_url: "https://site.test/page/mod2.js".into(),
            source: "export const val = 'B';".into(),
        }),
    );
    assert_eq!(log(&mut rt2), "turn2,rt2:B");
    assert_eq!(log(&mut rt1), "turn1"); // rt1 still pending, unaffected

    // 4. Resolve rt1
    rt1.supply_module(
        "https://site.test/page/mod1.js",
        Ok(FetchedModule {
            final_url: "https://site.test/page/mod1.js".into(),
            source: "export const val = 'A';".into(),
        }),
    );
    assert_eq!(log(&mut rt1), "turn1,rt1:A");

    // 5. Dropping rt1 leaves rt2 functioning normally
    drop(rt1);
    let val = rt2.evaluate_script("10 + 20").unwrap();
    assert!(matches!(val, JsValue::Number(n) if n == 30.0));
}

/// A runtime dropped or cancelled while a dynamic import is pending must cleanly
/// unwind its executor state, cancel pending futures, and release resources
/// without panicking, leaking memory, or triggering use-after-free.
#[test]
fn cancellation_and_teardown_during_pending_import_cleans_up_safely() {
    // Case A: Dynamic import is cancelled via error supply, runtime recovers and stays usable
    {
        let mut rt = runtime();
        rt.evaluate_script(
            "import('./cancel.js').then(function () { log.push('RESOLVED'); }, function (e) { log.push('cancelled:' + e.name); });",
        )
        .unwrap();
        assert_eq!(rt.take_module_requests(), vec!["https://site.test/page/cancel.js"]);

        // Supply error to reject the pending fetch
        rt.supply_module(
            "https://site.test/page/cancel.js",
            Err("Module fetch cancelled".to_string()),
        );
        assert_eq!(log(&mut rt), "cancelled:TypeError");

        // Runtime remains completely usable
        let res = rt.evaluate_script("'alive'").unwrap();
        assert!(matches!(res, JsValue::String(s) if s == "alive"));
    }

    // Case B: Runtime is dropped directly while dynamic import is in-flight
    {
        let mut rt = runtime();
        rt.evaluate_script(
            "import('./never_supplied.js').then(function (m) { log.push(m); });",
        )
        .unwrap();
        assert_eq!(rt.take_module_requests(), vec!["https://site.test/page/never_supplied.js"]);
        // Dropping the runtime with a pending import must not panic or trigger undefined behavior
        drop(rt);
    }
}
