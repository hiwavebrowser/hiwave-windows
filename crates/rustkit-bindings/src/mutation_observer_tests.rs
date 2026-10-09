//! MutationObserver (DOM §4.3): option validation, the records each DOM
//! write path queues, microtask delivery (one callback per observer with
//! all its records, in order), takeRecords, disconnect, subtree and
//! transient observers, several observers, and the pin that a page with no
//! observer pays nothing (web_mutation_observer.js).

use super::*;

const PAGE: &str = r#"<!DOCTYPE html><html><head><title>T</title></head>
<body><div id="root"><p id="a">A</p><p id="b">B</p></div><span id="out"></span></body></html>"#;

// `desc` names a node by id, or `#text:data`; `fmt` prints one record.
const HELPERS: &str = r#"
var log = [], calls = 0;
var root = document.getElementById('root');
function mk(id) { var e = document.createElement('i'); e.id = id; return e; }
function desc(n) {
    if (!n) return 'null';
    if (n.nodeType === 3) return '#text:' + n.data;
    return n.id || n.nodeName.toLowerCase();
}
function fmt(r) {
    var s = r.type + ' ' + desc(r.target);
    if (r.type === 'childList') {
        return s + ' +[' + [].map.call(r.addedNodes, desc).join(',') + '] -[' +
            [].map.call(r.removedNodes, desc).join(',') + '] ' +
            desc(r.previousSibling) + '..' + desc(r.nextSibling);
    }
    return s + ' ' + (r.attributeName === null ? '' : r.attributeName + ' ') + 'old=' + r.oldValue;
}
function watcher(name) {
    return function (recs) { calls++; log.push(name + '(' + recs.length + '): ' + recs.map(fmt).join(' | ')); };
}
"#;

fn bound() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap()))
        .unwrap();
    b.evaluate(HELPERS).unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(x) => x.to_string(),
        JsValue::Number(n) => format!("{n}"),
        JsValue::Undefined => "undefined".into(),
        JsValue::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

/// What the observers delivered, one line per callback call.
fn delivered(b: &DomBindings) -> String {
    ev(b, "log.join('\\n')")
}

#[test]
fn observe_validates_its_options_like_the_spec() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            r#"var mo = new MutationObserver(function () {}), r = [];
            function tryObs(t, o) { try { mo.observe(t, o); return 'ok'; } catch (e) { return e.name; } }
            r.push(tryObs(root, {}), tryObs(root, { subtree: true }),
                   tryObs(root, { attributes: false, attributeOldValue: true }),
                   tryObs(root, { attributes: false, attributeFilter: ['x'] }),
                   tryObs(root, { characterData: false, characterDataOldValue: true }),
                   tryObs(root, { attributeOldValue: true }), tryObs(root, { attributeFilter: ['x'] }),
                   tryObs(root, { characterDataOldValue: true }), tryObs(root, { childList: true }),
                   tryObs({}, { childList: true }), tryObs(root));
            try { MutationObserver(function () {}); r.push('ok'); } catch (e) { r.push(e.name); }
            try { new MutationObserver(); r.push('ok'); } catch (e) { r.push(e.name); }
            mo.disconnect();
            r.join()"#
        ),
        "TypeError,TypeError,TypeError,TypeError,TypeError,ok,ok,ok,ok,TypeError,TypeError,TypeError,TypeError"
    );
}

#[test]
fn child_list_records_are_delivered_as_one_microtask_after_the_script() {
    let b = bound();
    // Nothing is delivered while the script that wrote is still running.
    assert_eq!(
        ev(
            &b,
            r#"var mo = new MutationObserver(function (recs, o) {
                calls++;
                log.push(recs.length + ':' + (o === mo) + ':' + (this === mo) + ':' +
                    (recs[0] instanceof MutationRecord) + ':' + recs.map(fmt).join(' | '));
            });
            mo.observe(root, { childList: true });
            root.appendChild(mk('x'));
            root.insertBefore(mk('y'), document.getElementById('a'));
            root.removeChild(document.getElementById('b'));
            calls + ':' + log.length"#
        ),
        "0:0"
    );
    assert_eq!(
        delivered(&b),
        "3:true:true:true:childList root +[x] -[] b..null | childList root +[y] -[] null..a | childList root +[] -[b] a..x"
    );
    assert_eq!(ev(&b, "String(calls)"), "1");
}

#[test]
fn every_tree_write_path_queues_one_record_per_operation() {
    let b = bound();
    b.evaluate(
        r#"var mo = new MutationObserver(watcher('mo'));
        mo.observe(root, { childList: true });
        root.replaceChild(mk('r'), document.getElementById('b'));
        document.getElementById('a').remove();
        root.append(mk('c'), mk('d'));
        root.prepend(mk('e'));
        document.getElementById('c').before(mk('f'), mk('g'));
        document.getElementById('d').after(mk('h'));
        document.getElementById('r').replaceWith(mk('s'), 'txt');"#,
    )
    .unwrap();
    assert_eq!(
        delivered(&b),
        "mo(7): childList root +[r] -[b] a..null | childList root +[] -[a] null..r | \
         childList root +[c,d] -[] r..null | childList root +[e] -[] null..r | \
         childList root +[f,g] -[] r..c | childList root +[h] -[] d..null | \
         childList root +[s,#text:txt] -[r] e..f"
    );
    assert_eq!(
        ev(&b, "[].map.call(root.childNodes, desc).join()"),
        "e,s,#text:txt,f,g,c,d,h"
    );
}

#[test]
fn moving_a_node_records_its_removal_from_the_old_parent_first() {
    let b = bound();
    b.evaluate(
        r#"var mo = new MutationObserver(watcher('mo'));
        var out = document.getElementById('out');
        mo.observe(root, { childList: true });
        mo.observe(out, { childList: true });
        out.appendChild(document.getElementById('a'));
        var f = document.createDocumentFragment(); f.appendChild(mk('f1')); f.appendChild(mk('f2'));
        root.appendChild(f);"#,
    )
    .unwrap();
    assert_eq!(
        delivered(&b),
        "mo(3): childList root +[] -[a] null..b | childList out +[a] -[] null..null | \
         childList root +[f1,f2] -[] b..null"
    );
}

#[test]
fn inner_html_and_text_content_replace_all_children_in_one_record() {
    let b = bound();
    b.evaluate(
        r#"var mo = new MutationObserver(watcher('mo'));
        mo.observe(root, { childList: true });
        root.innerHTML = '<b id="n1"></b><b id="n2"></b>';
        root.textContent = 'hi';
        root.textContent = '';
        root.textContent = '';"#,
    )
    .unwrap();
    assert_eq!(
        delivered(&b),
        "mo(3): childList root +[n1,n2] -[a,b] null..null | childList root +[#text:hi] -[n1,n2] null..null | \
         childList root +[] -[#text:hi] null..null"
    );
}

#[test]
fn attribute_writes_from_every_path_are_recorded_with_old_values() {
    let b = bound();
    b.evaluate(
        r#"var a = document.getElementById('a');
        var mo = new MutationObserver(watcher('all'));
        var only = new MutationObserver(watcher('title'));
        mo.observe(a, { attributes: true, attributeOldValue: true });
        only.observe(a, { attributeFilter: ['title'] });
        a.setAttribute('title', 't1');
        a.setAttribute('TITLE', 't2');
        a.setAttribute('title', 't2');
        a.removeAttribute('nope');
        a.removeAttribute('title');
        a.toggleAttribute('hidden');
        a.classList.add('k');
        a.dataset.fooBar = '1';
        a.style.color = 'red';
        a.id = 'a2';"#,
    )
    .unwrap();
    assert_eq!(
        delivered(&b),
        "all(9): attributes a2 title old=null | attributes a2 title old=t1 | attributes a2 title old=t2 | \
         attributes a2 title old=t2 | attributes a2 hidden old=null | attributes a2 class old=null | \
         attributes a2 data-foo-bar old=null | attributes a2 style old=null | attributes a2 id old=a\n\
         title(4): attributes a2 title old=null | attributes a2 title old=null | attributes a2 title old=null | \
         attributes a2 title old=null"
    );
}

#[test]
fn character_data_writes_are_recorded_through_data_node_value_and_text_content() {
    let b = bound();
    b.evaluate(
        r#"var t = document.getElementById('a').firstChild;
        var deep = new MutationObserver(watcher('deep'));
        var shallow = new MutationObserver(watcher('shallow'));
        deep.observe(root, { characterDataOldValue: true, subtree: true });
        shallow.observe(root, { characterData: true });
        t.data = 'B'; t.nodeValue = 'C'; t.textContent = 'D';"#,
    )
    .unwrap();
    assert_eq!(
        delivered(&b),
        "deep(3): characterData #text:D old=A | characterData #text:D old=B | characterData #text:D old=C"
    );
}

#[test]
fn subtree_sees_descendants_and_removed_nodes_stay_observed_until_delivery() {
    let b = bound();
    b.evaluate(
        r#"var a = document.getElementById('a');
        var deep = new MutationObserver(watcher('deep'));
        var shallow = new MutationObserver(watcher('shallow'));
        deep.observe(root, { childList: true, attributes: true, subtree: true });
        shallow.observe(root, { attributes: true });
        a.appendChild(mk('x'));
        a.setAttribute('k', '1');
        root.removeChild(a);
        a.setAttribute('k', '2');"#,
    )
    .unwrap();
    assert_eq!(
        delivered(&b),
        "deep(4): childList a +[x] -[] #text:A..null | attributes a k old=null | \
         childList root +[] -[a] null..b | attributes a k old=null"
    );
    // The transient observer on the removed node ended with that delivery.
    b.evaluate("a.setAttribute('k', '3');").unwrap();
    assert_eq!(ev(&b, "String(calls)"), "1");
}

#[test]
fn take_records_empties_the_queue_and_disconnect_drops_it() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            r#"var mo = new MutationObserver(watcher('mo'));
            mo.observe(root, { childList: true });
            root.appendChild(mk('x'));
            var taken = mo.takeRecords();
            var again = mo.takeRecords().length;
            root.appendChild(mk('y'));
            mo.disconnect();
            root.appendChild(mk('z'));
            taken.length + ':' + fmt(taken[0]) + ':' + again"#
        ),
        "1:childList root +[x] -[] b..null:0"
    );
    assert_eq!(ev(&b, "String(calls)"), "0");
    // Observing again after disconnect starts a fresh registration.
    b.evaluate(
        "mo.observe(root, { childList: true }); root.removeChild(document.getElementById('z'));",
    )
    .unwrap();
    assert_eq!(delivered(&b), "mo(1): childList root +[] -[z] y..null");
}

#[test]
fn several_observers_get_their_own_records_in_creation_order() {
    let b = bound();
    b.evaluate(
        r#"var a = document.getElementById('a'), seen = [];
        var m1 = new MutationObserver(function (r) { seen.push(r[0]); watcher('m1')(r); });
        var m2 = new MutationObserver(function (r) { seen.push(r[0]); watcher('m2')(r); });
        var m3 = new MutationObserver(watcher('m3'));
        m3.observe(a, { attributes: true });
        m1.observe(root, { childList: true });
        m1.observe(root, { childList: true });
        m2.observe(root, { childList: true, subtree: true });
        root.appendChild(mk('x'));
        a.appendChild(mk('y'));
        a.setAttribute('k', 'v');"#,
    )
    .unwrap();
    assert_eq!(
        delivered(&b),
        "m1(1): childList root +[x] -[] b..null\n\
         m2(2): childList root +[x] -[] b..null | childList a +[y] -[] #text:A..null\n\
         m3(1): attributes a k old=null"
    );
    assert_eq!(ev(&b, "String(seen[0] !== seen[1])"), "true");
}

#[test]
fn a_callback_that_mutates_is_called_again_in_the_same_checkpoint() {
    let b = bound();
    b.evaluate(
        r#"var mo = new MutationObserver(function (recs) {
            watcher('mo')(recs);
            if (calls === 1) root.appendChild(mk('again'));
        });
        mo.observe(root, { childList: true });
        root.appendChild(mk('x'));"#,
    )
    .unwrap();
    assert_eq!(
        delivered(&b),
        "mo(1): childList root +[x] -[] b..null\nmo(1): childList root +[again] -[] x..null"
    );
}

#[test]
fn custom_element_reactions_still_run_beside_an_observer() {
    let b = bound();
    b.evaluate(
        r#"var ce = [];
        class XEl extends HTMLElement {
            static get observedAttributes() { return ['k']; }
            connectedCallback() { ce.push('connected'); }
            disconnectedCallback() { ce.push('disconnected'); }
            attributeChangedCallback(n, o, v) { ce.push(n + ':' + o + '>' + v); }
        }
        customElements.define('x-el', XEl);
        var mo = new MutationObserver(watcher('mo'));
        mo.observe(root, { childList: true, attributes: true, subtree: true, attributeOldValue: true });
        var x = document.createElement('x-el'); x.id = 'xe';
        root.appendChild(x);
        x.setAttribute('k', '1');
        x.remove();"#,
    )
    .unwrap();
    assert_eq!(ev(&b, "ce.join()"), "connected,k:null>1,disconnected");
    assert_eq!(
        delivered(&b),
        "mo(3): childList root +[xe] -[] b..null | attributes xe k old=null | childList root +[] -[xe] b..null"
    );
}

/// A page that never constructs a MutationObserver writes exactly as
/// before: no hook is installed, so each write pays one property check.
#[test]
fn a_document_with_no_observer_is_unchanged() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            r#"var a = document.getElementById('a');
            root.appendChild(mk('x'));
            root.insertBefore(mk('y'), a);
            root.replaceChild(mk('r'), document.getElementById('b'));
            root.append('t', mk('z'));
            a.setAttribute('title', 't'); a.classList.add('c'); a.dataset.q = '1'; a.style.color = 'red';
            a.firstChild.data = 'AA';
            document.getElementById('x').remove();
            document.getElementById('out').innerHTML = '<b>b</b>';
            document.getElementById('out').textContent = 'o';
            [typeof __rkDomHooks.mutation, root.outerHTML, document.getElementById('out').outerHTML].join('|')"#
        ),
        "undefined|<div id=\"root\"><i id=\"y\"></i><p class=\"c\" data-q=\"1\" id=\"a\" style=\"color: red;\" title=\"t\">AA</p><i id=\"r\"></i>t<i id=\"z\"></i></div>|<span id=\"out\">o</span>"
    );
    assert_eq!(ev(&b, "String(calls)"), "0");
}
