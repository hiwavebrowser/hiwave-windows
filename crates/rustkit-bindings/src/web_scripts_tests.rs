//! Portable pins for `web_scripts.js` (#712): HTML `createElementNS` and
//! `cloneNode` of a not-yet-started script prepare on insert; SVG scripts
//! and copies of already-started / `innerHTML` scripts do not.
//!
//! Headless engine coverage lives in `script_loading_tests` (network +
//! shield). These bindings-only cases prove the prepare markers without
//! macOS or `--features headless`.

use super::*;
use rustkit_dom::Document;
use std::rc::Rc;

fn bound(html: &str) -> DomBindings {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings
        .set_document(Rc::new(Document::parse_html(html).unwrap()))
        .unwrap();
    bindings
        .set_location(&Url::parse("https://site.test/page").unwrap())
        .unwrap();
    bindings
}

fn ev(bindings: &DomBindings, script: &str) -> String {
    match bindings.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(b) => b.to_string(),
        JsValue::Number(n) => n.to_string(),
        JsValue::Null => "null".to_string(),
        JsValue::Undefined => "undefined".to_string(),
        other => panic!("{script} evaluated to {other:?}"),
    }
}

#[test]
fn create_element_ns_html_script_runs_when_inserted() {
    let b = bound("<!DOCTYPE html><html><body></body></html>");
    assert_eq!(
        ev(
            &b,
            "window.mark = []; \
             var s = document.createElementNS('http://www.w3.org/1999/xhtml', 'script'); \
             s.textContent = \"window.mark.push('ns')\"; \
             document.body.appendChild(s); \
             mark.join()"
        ),
        "ns"
    );
}

#[test]
fn create_element_ns_svg_script_is_not_prepared() {
    let b = bound("<!DOCTYPE html><html><body></body></html>");
    assert_eq!(
        ev(
            &b,
            "window.mark = []; \
             var s = document.createElementNS('http://www.w3.org/2000/svg', 'script'); \
             s.textContent = \"window.mark.push('svg')\"; \
             document.body.appendChild(s); \
             mark.length + ',' + s.namespaceURI"
        ),
        "0,http://www.w3.org/2000/svg"
    );
}

#[test]
fn clone_of_a_fresh_script_runs_when_inserted() {
    let b = bound("<!DOCTYPE html><html><body></body></html>");
    assert_eq!(
        ev(
            &b,
            "window.mark = []; \
             var t = document.createElement('script'); \
             t.textContent = \"window.mark.push('clone')\"; \
             document.body.appendChild(t.cloneNode(true)); \
             mark.join()"
        ),
        "clone"
    );
}

#[test]
fn clone_of_a_started_or_innerhtml_script_never_runs() {
    let b = bound("<!DOCTYPE html><html><body></body></html>");
    assert_eq!(
        ev(
            &b,
            "window.mark = []; \
             var ran = document.createElement('script'); \
             ran.textContent = \"window.mark.push('once')\"; \
             document.body.appendChild(ran); \
             document.body.appendChild(ran.cloneNode(true)); \
             var host = document.createElement('div'); \
             host.innerHTML = '<script>window.mark.push(\"html\")</script>'; \
             document.body.appendChild(host.firstChild.cloneNode(true)); \
             mark.join()"
        ),
        "once"
    );
}

/// Why the engine's inserted-script path has no unparseable branch (20:50
/// (2) on #797's follow-ups): `web_scripts.js` resolves `src` with the URL
/// it captured at install (the `url` crate's parser, `web_url.rs`) and
/// fires `error` when that throws, so the queue only ever holds an `href`
/// the engine's `Url::parse` takes. Not even a page's own `URL` reaches it.
#[test]
fn an_unparseable_inserted_src_never_reaches_the_engine_queue() {
    let b = bound("<!DOCTYPE html><html><body></body></html>");
    ev(
        &b,
        "var bad = document.createElement('script'); \
         bad.src = 'https://cdn.example:99999/a.js?session=SECRET'; \
         document.body.appendChild(bad); \
         window.URL = function (u) { this.href = String(u); }; \
         var forged = document.createElement('script'); \
         forged.setAttribute('src', 'https://cdn.example:99999/b.js'); \
         document.body.appendChild(forged); 'ok'",
    );
    assert!(b.take_inserted_scripts().is_empty(), "an unparseable src was queued");
    ev(
        &b,
        "var good = document.createElement('script'); \
         good.src = '/c.js'; document.body.appendChild(good); 'ok'",
    );
    let queued = b.take_inserted_scripts();
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0].url, "https://site.test/c.js");
    assert!(queued.iter().all(|q| Url::parse(&q.url).is_ok()));
}
