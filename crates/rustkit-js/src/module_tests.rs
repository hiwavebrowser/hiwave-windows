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
