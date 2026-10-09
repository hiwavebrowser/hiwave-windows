//! Session history (`history.pushState`/`replaceState`/`state`/`length`,
//! traversal with `popstate`) and the anchor URL parts (HTML
//! §7.4 and the HTMLHyperlinkElementUtils mixin). Its own test module so it
//! does not collide with the other families' tests in `lib.rs`.

use super::*;

const PAGE: &str = r#"<!DOCTYPE html><html><body>
<a id="rel" href="../page?x=1#frag">rel</a><a id="abs" href="http://other.test:8080/p/q?s#h">abs</a>
<a id="none">none</a><a id="bad" href="http://[bad">bad</a></body></html>"#;

fn bound() -> DomBindings {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings
        .set_document(Rc::new(Document::parse_html(PAGE).unwrap()))
        .unwrap();
    bindings
        .set_location(&Url::parse("https://site.test/dir/index.html?a=1#top").unwrap())
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

/// Run queued tasks (popstate is fired from a task, not synchronously).
fn settle(bindings: &DomBindings) {
    bindings.run_timers(1000, 100).unwrap();
}

#[test]
fn push_state_updates_state_length_and_location() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var r = [history.length, String(history.state), 'state' in history]; \
             history.pushState({ n: 1, list: [1, 2] }, '', '/other/path?q=2#h'); \
             r.push(history.length, history.state.n, history.state.list.join('+'), \
                    location.href, location.pathname, location.search, location.hash, document.URL); \
             history.pushState(null, '', 'rel'); \
             r.push(history.length, String(history.state), location.href); \
             history.pushState(7, ''); \
             r.push(history.length, history.state, location.href); \
             r.join(',')"
        ),
        "1,null,true,2,1,1+2,https://site.test/other/path?q=2#h,/other/path,?q=2,#h,\
         https://site.test/other/path?q=2#h,3,null,https://site.test/other/rel,\
         4,7,https://site.test/other/rel"
    );
}

#[test]
fn state_is_a_structured_copy() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var o = { a: 1, d: new Date(5), nested: { x: [1] } }; o.self = o; \
             history.pushState(o, ''); o.a = 2; o.nested.x.push(2); \
             var s = history.state, r = [s === o, s.a, s.nested.x.length, s.self === s, \
                                         s.d instanceof Date, s.d.getTime(), history.state === s]; \
             try { history.pushState({ f: function () {} }, ''); r.push('no throw'); } \
             catch (e) { r.push(e.name, e instanceof DOMException); } \
             r.push(history.length); r.join(',')"
        ),
        "false,1,1,true,true,5,true,DataCloneError,true,2"
    );
}

#[test]
fn replace_state_keeps_length() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "history.replaceState({ k: 'v' }, '', '?b=2'); \
             var r = [history.length, history.state.k, location.href, location.search, location.hash]; \
             history.replaceState(null, '', null); r.push(location.href, String(history.state)); \
             r.join(',')"
        ),
        "1,v,https://site.test/dir/index.html?b=2,?b=2,,https://site.test/dir/index.html?b=2,null"
    );
}

#[test]
fn cross_origin_and_unparsable_urls_throw_security_error() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var r = []; \
             ['https://evil.test/x', 'http://site.test/x', 'https://site.test:444/x', 'http://[bad'].forEach(function (u) { \
               try { history.pushState(null, '', u); r.push('no throw'); } \
               catch (e) { r.push(e.name, e instanceof DOMException); } \
             }); \
             try { history.replaceState(null, '', 'https://evil.test/'); r.push('no throw'); } \
             catch (e) { r.push(e.name); } \
             try { history.pushState(); r.push('no throw'); } catch (e) { r.push(e.name); } \
             r.push(history.length, location.href); r.join(',')"
        ),
        "SecurityError,true,SecurityError,true,SecurityError,true,SecurityError,true,\
         SecurityError,TypeError,1,https://site.test/dir/index.html?a=1#top"
    );
}

#[test]
fn back_forward_go_fire_popstate_with_state() {
    let b = bound();
    ev(
        &b,
        "window.log = []; \
         window.addEventListener('popstate', function (e) { \
           log.push(e.type + ':' + JSON.stringify(e.state) + ':' + location.pathname + ':' + \
                    (e instanceof PopStateEvent) + ':' + (history.state === e.state)); \
         }); \
         history.pushState({ p: 1 }, '', '/one'); \
         history.pushState({ p: 2 }, '', '/two'); \
         history.back(); log.push('sync:' + location.pathname);",
    );
    settle(&b);
    assert_eq!(
        ev(&b, "log.join('|')"),
        "sync:/two|popstate:{\"p\":1}:/one:true:true"
    );
    ev(&b, "log = []; history.go(-1);");
    settle(&b);
    assert_eq!(
        ev(&b, "log.join('|') + '|' + history.length"),
        "popstate:null:/dir/index.html:true:true|3"
    );
    ev(&b, "log = []; history.forward(); history.go(5);");
    settle(&b);
    assert_eq!(ev(&b, "log.join('|')"), "popstate:{\"p\":1}:/one:true:true");
    // A push from the middle drops the forward entries.
    ev(
        &b,
        "log = []; history.pushState('x', '', '/three'); history.forward(); \
         window.onpopstate = function (e) { log.push('on:' + e.state); }; history.go(-2);",
    );
    settle(&b);
    assert_eq!(
        ev(&b, "log.join('|') + '|' + history.length + '|' + location.href"),
        "popstate:null:/dir/index.html:true:true|on:null|3|https://site.test/dir/index.html?a=1#top"
    );
}

#[test]
fn anchor_href_resolves_against_the_document_url() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var a = document.getElementById('rel'), r = []; \
             r.push(a.href, a.protocol, a.host, a.hostname, a.port, a.pathname, a.search, a.hash, \
                    a.origin, String(a)); \
             var x = document.getElementById('abs'); \
             r.push(x.href, x.protocol, x.host, x.hostname, x.port, x.pathname, x.search, x.hash, x.origin); \
             r.join(',')"
        ),
        "https://site.test/page?x=1#frag,https:,site.test,site.test,,/page,?x=1,#frag,\
         https://site.test,https://site.test/page?x=1#frag,\
         http://other.test:8080/p/q?s#h,http:,other.test:8080,other.test,8080,/p/q,?s,#h,\
         http://other.test:8080"
    );
}

#[test]
fn anchor_without_or_with_bad_href() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var n = document.getElementById('none'), d = document.getElementById('bad'); \
             [JSON.stringify(n.href), n.protocol, JSON.stringify(n.pathname), JSON.stringify(n.origin), \
              d.href, d.protocol, JSON.stringify(d.host)].join(',')"
        ),
        "\"\",:,\"\",\"\",http://[bad,:,\"\""
    );
}

#[test]
fn anchor_setters_write_the_href_attribute() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var a = document.createElement('a'), r = []; \
             a.href = '/x/y?z'; r.push(a.getAttribute('href'), a.href, a.pathname); \
             a.pathname = '/new'; a.search = 'k=v'; a.hash = 'h'; r.push(a.href, a.getAttribute('href')); \
             a.hostname = 'other.test'; a.port = '81'; a.protocol = 'http'; r.push(a.href, a.origin); \
             history.pushState(null, '', '/moved/'); var b2 = document.createElement('a'); \
             b2.setAttribute('href', 'z'); r.push(b2.href); \
             r.join(',')"
        ),
        "/x/y?z,https://site.test/x/y?z,/x/y,https://site.test/new?k=v#h,https://site.test/new?k=v#h,\
         http://other.test:81/new?k=v#h,http://other.test:81,https://site.test/moved/z"
    );
}

// ---- navigations script starts (the live app follows them) ----

const DOC: &str = "https://site.test/dir/index.html?a=1#top";

#[test]
fn assigning_location_asks_for_a_navigation() {
    let b = bound();
    // Binding the document and writing its URL are not requests.
    assert_eq!(b.take_navigation_requests(), Vec::<String>::new());

    ev(&b, "location.href = '../next?x=1'; 0");
    assert_eq!(b.take_navigation_requests(), ["https://site.test/next?x=1"]);
    // Taken once.
    assert_eq!(b.take_navigation_requests(), Vec::<String>::new());

    for (script, want) in [
        ("location.assign('/a')", "https://site.test/a"),
        (
            "location.replace('https://other.test/b')",
            "https://other.test/b",
        ),
        ("window.location = '/c'", "https://site.test/c"),
        ("document.location = '/d'", "https://site.test/d"),
    ] {
        b.set_location(&Url::parse(DOC).unwrap()).unwrap();
        ev(&b, &format!("{script}; 0"));
        assert_eq!(b.take_navigation_requests(), [want], "{script}");
    }
    // `window.location = url` used to replace the object with a string.
    assert_eq!(
        ev(&b, "typeof location + ',' + typeof location.assign"),
        "object,function"
    );
}

#[test]
fn what_is_not_a_navigation_asks_for_none() {
    let b = bound();
    // The same document with another fragment, a script URL, a URL that
    // does not parse, and the history API's own rewrites of `location`.
    ev(
        &b,
        "window.log = []; addEventListener('hashchange', function (e) { log.push(e.newURL); }); \
         location.href = '#other'; location.href = 'javascript:void 0'; location.href = 'http://[bad'; \
         history.pushState(null, '', '/pushed'); history.replaceState(null, '', '/replaced'); 0",
    );
    assert_eq!(b.take_navigation_requests(), Vec::<String>::new());
    // The fragment was a navigation inside the document.
    assert_eq!(
        ev(&b, "log.join()"),
        "https://site.test/dir/index.html?a=1#other"
    );
    // `location` reads the document's URL, not what script assigned.
    ev(&b, "location.href = '/away'; 0");
    assert_eq!(ev(&b, "location.href"), "https://site.test/replaced");
    assert_eq!(b.take_navigation_requests(), ["https://site.test/away"]);
    // A new document's URL, written by the engine.
    b.set_location(&Url::parse("https://site.test/elsewhere").unwrap())
        .unwrap();
    assert_eq!(b.take_navigation_requests(), Vec::<String>::new());
    assert_eq!(ev(&b, "location.href"), "https://site.test/elsewhere");
}

#[test]
fn a_script_click_on_a_link_follows_it() {
    let b = bound();
    ev(&b, "document.getElementById('abs').click(); 0");
    assert_eq!(
        b.take_navigation_requests(),
        ["http://other.test:8080/p/q?s#h"]
    );

    // From a descendant of the link, resolved against the document.
    ev(
        &b,
        "var a = document.getElementById('rel'), s = document.createElement('span'); a.appendChild(s); s.click(); 0",
    );
    assert_eq!(
        b.take_navigation_requests(),
        ["https://site.test/page?x=1#frag"]
    );

    // A cancelled click, a link with no href, one that does not parse, one
    // that opens elsewhere, a download, and a button inside a link (the
    // button has the activation) follow nothing.
    ev(
        &b,
        "a.addEventListener('click', function (e) { e.preventDefault(); }); a.click(); \
         document.getElementById('none').click(); document.getElementById('bad').click(); \
         var abs = document.getElementById('abs'); abs.setAttribute('target', '_blank'); abs.click(); \
         abs.removeAttribute('target'); abs.setAttribute('download', ''); abs.click(); \
         abs.removeAttribute('download'); var bt = document.createElement('button'); abs.appendChild(bt); bt.click(); 0",
    );
    assert_eq!(b.take_navigation_requests(), Vec::<String>::new());
}

#[test]
fn a_script_click_on_a_fragment_link_stays_in_the_document() {
    let b = bound();
    ev(
        &b,
        "window.log = []; addEventListener('hashchange', function (e) { log.push(e.newURL); }); \
         var f = document.createElement('a'); f.setAttribute('href', '#sec'); document.body.appendChild(f); f.click(); 0",
    );
    assert_eq!(b.take_navigation_requests(), Vec::<String>::new());
    assert_eq!(
        ev(
            &b,
            "log.join() + ' ' + location.hash + ' ' + history.length"
        ),
        "https://site.test/dir/index.html?a=1#sec #sec 2"
    );
}
