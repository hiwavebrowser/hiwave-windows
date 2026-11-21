//! Script elements load the way a browser loads them, however the page
//! gets them into the document.
//!
//! `data:` scripts are decoded where they are, never sent to the network or
//! the shield, and run as classic or module scripts by their `type`: from
//! the parser, inserted by script, or `import()`ed. Until 2026-10-16 each
//! `data:` request went through the request interceptor first, where the
//! shield logged "Failed to create request" for every one.
//!
//! Scripts inserted the ways a challenge page or a loader inserts them are
//! fetched and run: whatever made the element, however its `src` was set,
//! wherever and however it went into the document.

use super::error_page_script_tests::{engine, serve, text, Server};
use super::*;
use std::sync::Mutex;

/// Records every URL the loader asks the interceptor about, and blocks
/// every `data:` one: a `data:` script that runs never reached it.
struct Recorder(Arc<Mutex<Vec<String>>>);

impl rustkit_net::InterceptHandler for Recorder {
    fn intercept(&self, request: &rustkit_net::Request) -> rustkit_net::InterceptAction {
        self.0.lock().unwrap().push(request.url.to_string());
        if request.url.scheme() == "data" {
            rustkit_net::InterceptAction::Block
        } else {
            rustkit_net::InterceptAction::Allow
        }
    }
}

fn recording_engine() -> (Engine, EngineViewId, tokio::runtime::Runtime, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut interceptor = rustkit_net::RequestInterceptor::new();
    interceptor.add_handler(Arc::new(Recorder(seen.clone())));
    let mut engine = Engine::with_interceptor(EngineConfig::default(), Some(interceptor)).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    (engine, view, rt, seen)
}

fn page(body: &'static str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
    (200, "text/html", vec![], body.into())
}

fn paths(server: &Server) -> Vec<String> {
    server.requests().into_iter().map(|(p, _)| p).collect()
}

/// Every way a page hands the engine a `data:` script: the parser's classic
/// and module scripts (percent-encoded, plain and base64), the same inserted
/// by script, and `import()`. Each runs, by its type, and none is a request.
#[test]
fn data_url_scripts_run_without_the_network_or_the_shield() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/" => page(
                r#"<html><head>
<script>window.log = [];</script>
<script src="data:text/javascript,window.STICKY_CANARY=%22on%22;log.push('canary')"></script>
<script src="data:text/javascript;base64,bG9nLnB1c2goJ2Jhc2U2NCcpOw=="></script>
<script src="data:application/javascript;charset=utf-8,log.push(typeof document.currentScript.src === 'string' ? 'classic' : 'no')"></script>
<script type="module" src="data:text/javascript,let navPromise=null;async function getNavigation(){return navPromise||(navPromise=Promise.resolve('nav'))}window.getNavigation=getNavigation;log.push('module '+(typeof window.getNavigation))"></script>
<script type="module" src="data:text/javascript;base64,ZXhwb3J0IGNvbnN0IHg9MTsgbG9nLnB1c2goJ21vZHVsZTY0Jyk7"></script>
<script src="data:text/plain,log.push('plain classic')"></script>
<script src="data:text/javascript,log.push('frag')#ignored"></script>
</head><body><script>
var a = document.createElement('script');
a.src = 'data:text/javascript,log.push(%22inserted%20classic%20%22%2B(document.currentScript%3D%3D%3Da))';
a.onload = function () { log.push('load classic'); };
document.head.appendChild(a);
var m = document.createElement('script');
m.type = 'module';
m.src = 'data:text/javascript,log.push("inserted module " + (document.currentScript === null))';
m.onload = function () { log.push('load module'); };
document.head.appendChild(m);
var b = document.createElement('script');
b.setAttribute('src', 'data:text/javascript;base64,' + btoa("log.push('inserted base64')"));
document.body.appendChild(b);
import('data:text/javascript,export default 42#x').then(function (ns) { log.push('import ' + ns.default); },
  function (e) { log.push('import failed ' + e); });
import('data:text/plain,export default 1').then(function () { log.push('plain module ran'); },
  function () { log.push('plain module refused'); });
</script></body></html>"#,
            ),
            _ => (404, "text/plain", vec![], "".into()),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt, intercepted) = recording_engine();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    rt.block_on(engine.pump_live(view, 100));
    let log = text(&mut engine, view, "log.join(' | ')");
    for want in [
        "canary",
        "base64",
        "classic",
        "plain classic",
        // A fragment is not part of the script.
        "frag",
        "module function",
        "module64",
        "inserted classic true",
        "load classic",
        "inserted module true",
        "load module",
        "inserted base64",
        "import 42",
        // A module must be served as JavaScript, from data: as from http.
        "plain module refused",
    ] {
        assert!(log.contains(&format!("{want} |")) || log.ends_with(&format!("{want}\")")), "{want:?} missing from {log}");
    }
    assert_eq!(text(&mut engine, view, "String(window.STICKY_CANARY)"), r#"String("on")"#);
    let data: Vec<String> = intercepted.lock().unwrap().iter().filter(|u| u.starts_with("data:")).cloned().collect();
    assert!(data.is_empty(), "data: scripts reached the interceptor: {data:?}");
    assert_eq!(paths(&server), vec!["/"], "no data: script is a request");
}

/// A Cloudflare-style managed challenge: a 403 (`cf-mitigated: challenge`)
/// whose inline script sets `window._cf_chl_opt`, rewrites the URL with
/// `history.replaceState`, then inserts the orchestrating script. Each case
/// inserts it a different way; in a browser every one of them fetches it.
/// `{ray}` is the case's number, so the request says which case asked.
const INSERTIONS: &[(&str, &str)] = &[
    (
        "createElement, .src, onload, getElementsByTagName('head')[0].appendChild",
        "var cpo = document.createElement('script');\
         cpo.src = '/cdn-cgi/challenge-platform/h/g/orchestrate/chl_page/v1?ray={ray}';\
         window._cf_chl_opt.cOgUHash = location.hash === '' && location.href.indexOf('#') !== -1 ? '#' : location.hash;\
         window._cf_chl_opt.cOgUQuery = location.search === '' && location.href.slice(0, location.href.length - window._cf_chl_opt.cOgUHash.length).indexOf('?') !== -1 ? '?' : location.search;\
         if (window.history && window.history.replaceState) {\
           var ogU = location.pathname + window._cf_chl_opt.cOgUQuery + window._cf_chl_opt.cOgUHash;\
           history.replaceState(null, null, \"\\/p\\/{ray}?__cf_chl_rt_tk=tok-1700000000-1.0.1.1-abc\" + window._cf_chl_opt.cOgUHash);\
           cpo.onload = function () { history.replaceState(null, null, ogU); };\
         }\
         document.getElementsByTagName('head')[0].appendChild(cpo);",
    ),
    (
        "setAttribute('src'), async, nonce, document.head.append",
        "var s = document.createElement('script'); s.setAttribute('src', '/orchestrate?ray={ray}');\
         s.async = true; s.setAttribute('nonce', 'abc123'); s.nonce = 'abc123'; document.head.append(s);",
    ),
    (
        "currentScript.parentNode.insertBefore(s, currentScript)",
        "var s = document.createElement('script'); s.src = '/orchestrate?ray={ray}';\
         var me = document.currentScript; me.parentNode.insertBefore(s, me);",
    ),
    (
        "currentScript.after(s)",
        "var s = document.createElement('script'); s.src = '/orchestrate?ray={ray}'; document.currentScript.after(s);",
    ),
    (
        "insertAdjacentElement('afterend') on a specific element",
        "var s = document.createElement('script'); s.src = '/orchestrate?ray={ray}';\
         document.getElementById('challenge-body-text').insertAdjacentElement('afterend', s);",
    ),
    (
        "defer, document.body.prepend",
        "var s = document.createElement('script'); s.defer = true; s.src = '/orchestrate?ray={ray}'; document.body.prepend(s);",
    ),
    (
        "replaceChild for a placeholder element",
        "var s = document.createElement('script'); s.src = '/orchestrate?ray={ray}';\
         var p = document.getElementById('slot'); p.parentNode.replaceChild(s, p);",
    ),
    (
        "inserted empty, src set once connected",
        "var s = document.createElement('script'); document.head.appendChild(s); s.src = '/orchestrate?ray={ray}';",
    ),
    (
        "in a DocumentFragment, fragment appended",
        "var f = document.createDocumentFragment(); var s = document.createElement('script');\
         s.src = '/orchestrate?ray={ray}'; f.appendChild(s); document.body.appendChild(f);",
    ),
    (
        "createElementNS(XHTML, 'script')",
        "var s = document.createElementNS('http://www.w3.org/1999/xhtml', 'script');\
         s.src = '/orchestrate?ray={ray}'; document.head.appendChild(s);",
    ),
    (
        "cloneNode of a script not yet inserted",
        "var t = document.createElement('script'); t.src = '/orchestrate?ray={ray}';\
         document.head.appendChild(t.cloneNode(true));",
    ),
];

/// A `<noscript>` meta refresh to the no-JavaScript fallback: with
/// scripting on it is text, not a refresh.
const NOSCRIPT: usize = 1000;

fn challenge(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
    if path.starts_with("/cdn-cgi/") || path == "/orchestrate" {
        return (
            200,
            "text/javascript",
            vec![],
            "window.orchestrated = (window.orchestrated || 0) + 1; window.orchestratedBy = document.currentScript && document.currentScript.src;".into(),
        );
    }
    if path == "/nojs" {
        return (200, "text/html", vec![], "<title>no js</title>".into());
    }
    let Some(case) = path.strip_prefix("/p/").and_then(|n| n.parse::<usize>().ok()) else {
        return (404, "text/plain", vec![], "".into());
    };
    let (script, noscript) = if case == NOSCRIPT {
        (INSERTIONS[0].1, r#"<noscript><meta http-equiv="refresh" content="0; url=/nojs"></noscript>"#)
    } else {
        (INSERTIONS[case].1, "")
    };
    let body = format!(
        r#"<!DOCTYPE html><html lang="en-US"><head><title>Just a moment...</title>
<meta http-equiv="Content-Type" content="text/html; charset=UTF-8"><meta name="robots" content="noindex,nofollow">
<meta name="viewport" content="width=device-width,initial-scale=1">{noscript}<meta http-equiv="refresh" content="360"></head>
<body><div class="main-wrapper" role="main"><div class="main-content"><h1>example.test</h1>
<p id="challenge-body-text">Verifying you are human.</p><span id="slot"></span>
<noscript><div class="h2"><span id="challenge-error-text">Enable JavaScript and cookies to continue</span></div></noscript></div></div>
<script nonce="abc123">(function(){{window._cf_chl_opt = {{cvId: '3', cZone: 'example.test', cType: 'managed', cRay: '{case}', cH: 'h', cFPWv: 'b'}};{script}}}());</script>
</body></html>"#,
        script = script.replace("{ray}", &case.to_string()),
    );
    // A &'static header list: the handler's signature.
    (403, "text/html", vec![("cf-mitigated", "challenge")], body)
}

/// Every insertion above fetches the orchestrating script once and runs it
/// as the element's script; a `<noscript>` refresh is not a navigation.
#[test]
fn a_challenge_page_fetches_the_script_it_inserts_however_it_inserts_it() {
    let server = serve(challenge);
    let mut failures = Vec::new();
    let cases = (0..INSERTIONS.len()).chain([NOSCRIPT]);
    for case in cases {
        let name = INSERTIONS.get(case).map(|c| c.0).unwrap_or("<noscript><meta refresh> beside the real pattern");
        let (mut engine, view, rt) = engine();
        let before = server.requests().len();
        rt.block_on(engine.load_url(view, server.url(&format!("/p/{case}")))).expect("load");
        rt.block_on(engine.pump_live(view, 100));
        let fetched: Vec<String> =
            server.requests()[before..].iter().map(|(p, _)| p.clone()).filter(|p| !p.starts_with("/p/")).collect();
        let ran = text(&mut engine, view, "String(window.orchestrated)");
        let nav = engine.take_script_navigation(view);
        let fetched_ok = fetched.len() == 1 && fetched[0].ends_with(&format!("ray={case}"));
        let nav_ok = nav.is_none();
        if !(fetched_ok && ran == r#"String("1")"# && nav_ok) {
            failures.push(format!("{name}: fetched {fetched:?}, orchestrated {ran}, navigation {nav:?}"));
        }
    }
    assert!(failures.is_empty(), "insertions that did not load:\n{}", failures.join("\n"));
}

/// `innerHTML` never runs the scripts it parses, in a browser or here.
#[test]
fn a_script_set_through_inner_html_is_not_fetched() {
    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/" => page(
                r#"<div id="d"></div><script>document.getElementById('d').innerHTML = '<script src="/never.js"><\/script>';</script>"#,
            ),
            _ => (200, "text/javascript", vec![], "window.never = 1;".into()),
        }
    }
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
    rt.block_on(engine.pump_live(view, 100));
    assert_eq!(paths(&server), vec!["/"]);
    assert_eq!(text(&mut engine, view, "String(window.never)"), r#"String("undefined")"#);
}

