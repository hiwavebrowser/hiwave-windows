//! What the hottest DOM readers cost, counted in VM opcodes (not time): Speedometer reads `id`, `className`,
//! `children`, `firstElementChild` and the like per element per step. A count cannot regress without a failing
//! test.
use super::*;

fn grab_total(rep: &str) -> u64 {
    for l in rep.lines() {
        if let Some(r) = l.strip_prefix("vm stats: ") {
            return r.split_whitespace().next().unwrap().parse().unwrap();
        }
    }
    panic!("no vm stats line in {rep}");
}

/// Opcodes one execution of `op` costs, net of the empty loop.
fn per_op(op: &str) -> f64 {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><body><div id=root><p id=a class='row r1' data-i=5>hello</p><p id=b>two</p><p id=c>three</p></div></body></html>").unwrap(),
    ))
    .unwrap();
    b.evaluate("var root = document.getElementById('root'); var r2 = document.getElementById('a'); var x = 0; 'ok'").unwrap();
    let iters = 1000u64;
    let run = |body: &str| -> u64 {
        let before = grab_total(&rustkit_js::vm_stats_report());
        b.evaluate(&format!("(function(){{ for (var i = 0; i < {iters}; i++) {{ {body} }} return 'ok' }})()")).unwrap();
        grab_total(&rustkit_js::vm_stats_report()) - before
    };
    let empty = run("");
    let cost = run(op);
    (cost as f64 - empty as f64) / iters as f64
}

const OPS: &[(&str, &str, f64)] = &[
    ("el.id", "x = r2.id", 60.0),
    ("el.id = v", "r2.id = 'a'", 100.0),
    ("el.className", "x = r2.className", 60.0),
    ("el.className = v", "r2.className = 'row r1'", 100.0),
    ("el.getAttribute", "x = r2.getAttribute('data-i')", 65.0),
    ("el.nodeType", "x = r2.nodeType", 65.0),
    ("el.parentNode", "x = r2.parentNode", 100.0),
    ("el.nextSibling", "x = r2.nextSibling", 100.0),
    ("el.firstChild", "x = root.firstChild", 100.0),
    ("el.children", "x = root.children", 100.0),
    ("el.children[1]", "x = root.children[1]", 102.0),
    ("el.childNodes", "x = root.childNodes", 100.0),
    ("el.firstElementChild", "x = root.firstElementChild", 120.0),
    ("el.childElementCount", "x = root.childElementCount", 120.0),
];

/// The measuring half, run in a child process (the VM's counter switch is read once per process).
#[test]
#[ignore]
fn dom_accessor_cost_child() {
    for (name, op, _) in OPS {
        println!("COST|{name}|{:.1}", per_op(op));
    }
}

/// Budgets are the measured cost with a little room. Before (develop bfb001b4): id 66, id = 393, className 66,
/// className = 403, getAttribute 56, nodeType 59, parentNode / nextSibling / firstChild 94, children 455,
/// children[1] 457, childNodes 281, firstElementChild 508, childElementCount 506. The plain readers were already at
/// the floor (a getter call and one native call); the gains are the two setters and the collections.
#[test]
fn dom_reader_operations_stay_cheap() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "dom_accessor_cost_tests::dom_accessor_cost_child", "--ignored", "--nocapture", "--test-threads=1"])
        .env("RUSTKIT_VM_STATS", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let mut over = Vec::new();
    let mut seen = 0;
    for l in text.lines().filter_map(|l| l.find("COST|").map(|i| &l[i + 5..])) {
        let (name, n) = l.split_once('|').unwrap();
        let n: f64 = n.parse().unwrap();
        let budget = OPS.iter().find(|o| o.0 == name).unwrap().2;
        println!("COST {name:<24} {n:>8.1} (budget {budget})");
        seen += 1;
        if n > budget {
            over.push(format!("{name}: {n:.1} > {budget}"));
        }
    }
    assert_eq!(seen, OPS.len(), "child output: {text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(over.is_empty(), "{over:?}");
}

fn ev(script: &str) -> String {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html("<html><body></body></html>").unwrap())).unwrap();
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

/// Answers read from Chrome for Testing 148.0.7778.216 (ac_chrome.mjs).
#[test]
fn collections_and_attribute_accessors_answer_as_chrome_148_does() {
    let got = ev(r#"
        var o = [];
        var root = document.createElement('div'); root.innerHTML = 'a<p id=x class="k  l">1</p>b<span>2</span><!--c-->';
        o.push([root.children.length, root.childNodes.length, root.childElementCount, root.firstElementChild.id, root.lastElementChild.localName].join(','));
        o.push(root.children === root.children, root.childNodes === root.childNodes);
        o.push([Object.getPrototypeOf(root.children) === HTMLCollection.prototype, Object.getPrototypeOf(root.childNodes) === NodeList.prototype].join(','));
        var c = root.children; c[0] = 'zz'; o.push(typeof root.children[0] + ',' + (root.children[0] === c[0]));
        c.expando = 1; o.push(root.children.expando);
        var e = document.createElement('div'); o.push([e.children.length, e.firstElementChild, e.childElementCount, e.firstChild].join(','));
        var f = document.createDocumentFragment(); f.append('t', document.createElement('b'), 'u');
        o.push([f.children.length, f.childElementCount, f.firstElementChild.localName, f.childNodes.length].join(','));
        o.push(document.implementation.createHTMLDocument('t').children.length);
        var x = document.createElement('div');
        x.id = 5; o.push(x.getAttribute('id')); x.id = null; o.push(x.id); x.id = undefined; o.push(x.id);
        x.className = 7; o.push(x.className, document.createElement('p').className === '');
        var y = document.createElement('div'); y.id = 'q'; document.body.append(y);
        o.push(!!document.getElementById('q')); y.id = 'r';
        o.push([document.getElementById('q'), !!document.getElementById('r'), !!document.querySelector('#r')].join(','));
        o.join('|')"#);
    let want = "2,5,2,x,span|true|true|true,true|object,true|1|0,,0,|1,1,b,3|1|5|null|undefined|7|true|true|,true,true";
    assert_eq!(got, want);
}

/// Chrome's `children` and `childNodes` are live: a collection held across a change follows it. Here a held one
/// stays the snapshot it was, and the next read of `el.children` is a new object (Chrome: the same one). Named
/// so that a change to this is deliberate.
#[test]
fn known_divergence_a_held_collection_does_not_follow_a_change() {
    assert_eq!(
        ev("var q = document.createElement('div'), held = q.children, n = held.length; q.append(document.createElement('a')); \
            [n, held.length, q.children.length, held === q.children].join(',')"),
        "0,0,1,false"
    );
}

/// A change in the child list shows at the next read, however it was made.
#[test]
fn a_read_after_each_kind_of_child_change_sees_it() {
    assert_eq!(
        ev(r#"var q = document.createElement('div'), r = [], a = document.createElement('a'), b = document.createElement('b');
            r.push(q.children.length); q.append(a); r.push(q.children.length, q.firstElementChild === a);
            q.insertBefore(b, a); r.push(q.firstElementChild === b, q.lastElementChild === a, q.childElementCount);
            q.removeChild(b); r.push(q.children.length, q.childNodes.length);
            q.append('text'); r.push(q.childNodes.length, q.children.length);
            q.innerHTML = '<i></i><i></i>'; r.push(q.children.length, q.childNodes.length);
            var z = document.createElement('div'); z.append(q.firstElementChild); r.push(q.children.length, z.children.length);
            q.textContent = ''; r.push(q.children.length, q.firstElementChild);
            r.join(',')"#),
        "0,1,true,true,true,2,1,1,2,1,2,2,1,1,0,"
    );
}

/// A page's own `String.prototype.split`, `Object.defineProperty`, or `Element.prototype.setAttribute` changes
/// nothing the readers answer.
#[test]
fn readers_do_not_run_page_replaceable_built_ins() {
    let got = ev(r#"
        var q = document.createElement('div'); q.innerHTML = '<p></p>x<p></p>'; document.body.append(q);
        q.firstChild.nextSibling.nextSibling; // the children's wrappers exist before the page's replacement (`wrap` is not this change's)
        var keep = [String.prototype.split, Object.defineProperty, Element.prototype.setAttribute, Element.prototype.getAttribute, Array.prototype.filter];
        var hits = [], names = ['split','defineProperty','setAttribute','getAttribute','filter']; function mk(n) { return function () { hits[hits.length] = n; return undefined; }; }
        String.prototype.split = mk('split'); Object.defineProperty = mk('defineProperty'); Element.prototype.setAttribute = mk('setAttribute');
        Element.prototype.getAttribute = mk('getAttribute'); Array.prototype.filter = mk('filter');
        q.id = 'k'; q.className = 'm'; var out = [q.id, q.className, q.children.length, q.childNodes.length, q.childElementCount]; var seen = hits.length, which = hits.slice();
        String.prototype.split = keep[0]; Object.defineProperty = keep[1]; Element.prototype.setAttribute = keep[2];
        Element.prototype.getAttribute = keep[3]; Array.prototype.filter = keep[4];
        out.join('|') + '|' + seen + ':' + which.join(',')"#);
    assert_eq!(got, "k|m|2|3|2|0:");
}

/// Chrome 148 (ac_chrome2.mjs): a collection's index is `{writable: false, enumerable: true, configurable: true}` and
/// redefining one throws TypeError. Here the index is non-configurable, so a redefinition throws too; a define at an
/// index past the end succeeds (Chrome throws), and `configurable` reads false. The engine's own readers do not
/// read the page's collection at all.
#[test]
fn a_page_cannot_change_what_the_engines_own_readers_answer() {
    let got = ev(r#"var d = document.createElement('div'); d.innerHTML = '<i></i><b></b>'; var c = d.children, r = [];
        function t(f) { try { f(); return 'ok'; } catch (e) { return e.name; } }
        r.push(t(function () { Object.defineProperty(c, 0, { value: 42 }); }));
        r.push(t(function () { Object.defineProperty(c, 0, { value: c[0], writable: false, enumerable: true, configurable: true }); }));
        r.push(t(function () { 'use strict'; delete c[0]; }));
        var dd = Object.getOwnPropertyDescriptor(c, 0); r.push([dd.writable, dd.enumerable, dd.configurable].join());
        r.push(Object.keys(c).join('/'), d.firstElementChild.localName, d.lastElementChild.localName, d.childElementCount);
        var n = d.childNodes; r.push(t(function () { Object.defineProperty(n, 0, { value: 42 }); }));
        c.expando = 1; r.push(d.children.expando);
        r.join('|')"#);
    assert_eq!(got, "TypeError|TypeError|TypeError|false,true,false|0/1|i|b|2|TypeError|1");
}

/// Engine-owned records inherit nothing: a field a page puts on `Object.prototype` is not read as one of theirs.
#[test]
fn the_per_element_record_inherits_nothing() {
    let got = ev(r#"var fake = { length: 1, 0: 'FAKE' }, hits = 0;
        Object.defineProperty(Object.prototype, 'kidsE', { configurable: true, get: function () { hits++; return { ids: '', list: fake, items: fake, n: 1 }; }, set: function (v) { hits += 100; } });
        Object.defineProperty(Object.prototype, 'kidsN', { configurable: true, get: function () { hits++; return { ids: '', list: fake, items: fake, n: 1 }; }, set: function (v) { hits += 100; } });
        var d = document.createElement('div');
        var out = [d.children === fake, d.childNodes === fake, d.firstElementChild, d.lastElementChild, d.childElementCount, d.children.length];
        d.append(document.createElement('a')); out.push(d.firstElementChild.localName, d.children.length, hits);
        delete Object.prototype.kidsE; delete Object.prototype.kidsN;
        out.join('|')"#);
    assert_eq!(got, "false|false|||0|0|a|1|0");
}

/// A write through the native setters is seen by every reader that depends on the attribute.
#[test]
fn the_native_id_and_class_setters_update_what_depends_on_them() {
    let got = ev(r#"var e = document.createElement('div'); e.id = 'a'; document.body.append(e); var r = [];
        e.id = 'n'; r.push(document.getElementById('a') === null, document.getElementById('n') === e, document.querySelector('#n') === e, document.querySelector('#a') === null);
        e.className = 'k'; r.push(document.querySelector('.k') === e, document.getElementsByClassName('k').length, e.matches('.k'));
        e.id = 'm'; r.push(document.getElementById('n') === null, document.getElementById('m') === e);
        e.className = ''; r.push(document.querySelector('.k') === null, document.getElementsByClassName('k').length, e.matches('.k'));
        r.join('|')"#);
    assert_eq!(got, "true|true|true|true|true|1|true|true|true|true|0|false");
}
