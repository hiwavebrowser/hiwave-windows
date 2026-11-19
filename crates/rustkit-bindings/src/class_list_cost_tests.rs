//! What `classList` operations cost, counted in VM opcodes (not time): MotionMark and Speedometer call
//! `classList.add/remove/toggle/contains` per element per frame. A count cannot regress without a
//! failing test; a timing could.
use super::*;

fn grab_total(rep: &str) -> u64 {
    for l in rep.lines() {
        if let Some(r) = l.strip_prefix("vm stats: ") {
            return r.split_whitespace().next().unwrap().parse().unwrap();
        }
    }
    panic!("no vm stats line in {rep}");
}

/// Opcodes one execution of `op` costs, net of the empty loop, over `iters` iterations.
fn per_op(op: &str, setup: &str) -> f64 {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><body><div id=root><p id=a class='row r1' data-i=5>hello</p><p id=b>two</p></div></body></html>").unwrap(),
    ))
    .unwrap();
    b.evaluate(&format!("var r2 = document.getElementById('a'); var x = 0; {setup}; 'ok'")).unwrap();
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
    ("add of a token that is there", "r2.classList.add('r1')", 410.0),
    ("add then remove a token", "r2.classList.add('zz'); r2.classList.remove('zz')", 920.0),
    ("remove of a token that is not there", "r2.classList.remove('nope')", 410.0),
    ("toggle on then off", "r2.classList.toggle('t')", 440.0),
    ("contains", "x = r2.classList.contains('r1')", 235.0),
    ("add of two tokens", "r2.classList.add('r1', 'r2')", 500.0),
];

/// The measuring half: run in a child process, because the VM's counter switch (`RUSTKIT_VM_STATS`) is read
/// once per process and the other tests here start runtimes first.
#[test]
#[ignore]
fn class_list_cost_child() {
    for (name, op, _) in OPS {
        println!("COST|{name}|{:.1}", per_op(op, ""));
    }
}

/// Budgets are the measured cost after the change, with a little room; the same operations cost 771 / 1707 /
/// 888 / 772 / 271 / 849 opcodes before (develop 9f5cfca4).
#[test]
fn class_list_operations_stay_cheap() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "class_list_cost_tests::class_list_cost_child", "--ignored", "--nocapture", "--test-threads=1"])
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
        println!("COST {name:<38} {n:>8.1} (budget {budget})");
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

/// Every answer here was read from Chrome for Testing 148.0.7778.216 (cl_chrome.mjs, cl_chrome2.mjs).
#[test]
fn class_list_answers_as_chrome_148_does() {
    let got = ev(r#"
        var out = [];
        function mk(c) { var e = document.createElement('div'); if (c !== null) e.setAttribute('class', c); return e; }
        function t(f) { try { return 'ok: ' + String(f()); } catch (e) { return e.name; } }
        var e = mk(null); e.classList.add(); out.push(e.hasAttribute('class'));
        e = mk(null); e.classList.remove(); out.push(e.hasAttribute('class'));
        e = mk(null); e.classList.remove('x'); out.push(e.hasAttribute('class'));
        e = mk('  a   b  a '); e.classList.add('a'); out.push(e.getAttribute('class'));
        e = mk('  a   b  a '); e.classList.add(); out.push(e.getAttribute('class'));
        e = mk('  a   b  a '); e.classList.remove('zz'); out.push(e.getAttribute('class'));
        e = mk('  a   b  a '); out.push(e.classList.toggle('a', true), e.getAttribute('class'));
        e = mk('  a   b  a '); out.push(e.classList.toggle('q', false), e.getAttribute('class'));
        e = mk(''); e.classList.add('x'); out.push(e.getAttribute('class'));
        e = mk('a'); e.classList.add('b', 'c'); out.push(e.getAttribute('class'));
        e = mk('a'); out.push(t(function () { e.classList.add('b', ''); }), e.getAttribute('class'));
        e = mk('a'); out.push(t(function () { e.classList.add('b c'); }), e.getAttribute('class'));
        e = mk('a'); e.classList.add(undefined); out.push(e.getAttribute('class'));
        e = mk('a b'); out.push(e.classList.replace('x', 'y'), e.getAttribute('class'));
        e = mk('a\tb\nc'); out.push(e.classList.length, e.classList.contains('b'), e.classList.value === 'a\tb\nc');
        e = mk('a b'); var L = e.classList; e.setAttribute('class', 'z y'); out.push(L.length, L[0], L.contains('y'));
        e = mk('a'); out.push(e.classList === e.classList);
        e = mk('a b'); out.push(e.classList[0], e.classList[5], e.classList['01'], e.classList[-1]);
        e = mk('a b'); var args = [];
        e.classList.forEach(function (v, i, l) { args.push(v + i + (l === e.classList) + (this == 7)); }, 7); out.push(args.join(','));
        e = mk('a b c'); var seen = [];
        e.classList.forEach(function (v, i) { seen.push(v); if (i === 0) e.classList.remove('b'); }); out.push(seen.join(','));
        e = mk('a'); out.push(t(function () { e.classList.forEach(1); }));
        out.push(t(function () { DOMTokenList.prototype.add.call({}, 'x'); }));
        e = mk(null); e.classList.value = '  q  r '; out.push(e.getAttribute('class'), e.classList.length);
        e = mk('a b c'); out.push(e.classList.replace('a', 'c'), e.className);
        e = mk('a'); e.classList.add('b', 'b'); out.push(e.className);
        e = mk('a'); e.classList.remove('a'); out.push(e.getAttribute('class') === '');
        out.push(t(function () { mk('a').classList.toggle(''); }), mk('a b').classList.toString());
        out.join('|')"#);
    let want = "true|false|false|a b|a b|a b|true|  a   b  a |false|  a   b  a |x|a b c|SyntaxError|a|InvalidCharacterError|a|a undefined|false|a b|3|true|true|2|z|true|true|a||||a0truetrue,b1truetrue|a,c|TypeError|TypeError|  q  r |2|true|c b|a b|true|SyntaxError|a b";
    assert_eq!(got, want);
}

/// A page that replaces the built-ins the list uses, or the element's own attribute methods, changes nothing.
/// (`String.prototype.charCodeAt` is left out: the shared attribute-write path after the list's own code
/// (`setData`'s change hooks) calls it once per write; the list's own uses are the install-time copy.)
#[test]
fn class_list_does_not_run_page_replaceable_built_ins() {
    let got = ev(r#"
        var e = document.createElement('div'); e.className = ' a  b ';
        var L = e.classList; L.add('a'); var calls = 0;
        var f = document.createElement('div'); f.setAttribute('class', 'x  y'); var fl = f.classList;
        var log = []; function mkb(n) { return function () { calls++; log.push(n); return undefined; }; }
        var keep = [String.prototype.split, String.prototype.slice, String.prototype.charCodeAt, Array.prototype.indexOf, Array.prototype.join, Array.prototype.forEach, Array.prototype.values, RegExp.prototype.test, Function.prototype.call, Reflect.get, Element.prototype.setAttribute, Element.prototype.getAttribute, Element.prototype.hasAttribute];
        String.prototype.split = mkb('split'); String.prototype.slice = mkb('slice');
        Array.prototype.indexOf = mkb('indexOf'); Array.prototype.join = mkb('join'); Array.prototype.forEach = mkb('forEach'); Array.prototype.values = mkb('values');
        RegExp.prototype.test = mkb('test'); var NumKeep = Number; Number = mkb('Number'); Function.prototype.call = mkb('call'); Reflect.get = mkb('Reflect.get');
        Element.prototype.setAttribute = mkb('setAttribute'); Element.prototype.getAttribute = mkb('getAttribute'); Element.prototype.hasAttribute = mkb('hasAttribute');
        fl.add('z'); fl.remove('x'); fl.toggle('w'); fl.replace('y', 'v');
        var seen = []; fl.forEach(function (v) { seen.push(v); });
        var n = calls, has = fl.contains('z'), len = fl.length, zero = fl[0]; var names = log.slice();
        Element.prototype.setAttribute = keep[10]; Element.prototype.getAttribute = keep[11]; Element.prototype.hasAttribute = keep[12];
        String.prototype.split = keep[0]; String.prototype.slice = keep[1]; Array.prototype.indexOf = keep[3];
        Array.prototype.join = keep[4]; Array.prototype.forEach = keep[5]; Array.prototype.values = keep[6]; RegExp.prototype.test = keep[7];
        Function.prototype.call = keep[8]; Reflect.get = keep[9]; Number = NumKeep;
        var before = [f.getAttribute('class'), has, len, zero, seen.length, e.getAttribute('class'), n + ':' + names.join(',')];
        before.join('|')"#);
    assert_eq!(got, "v z w|true|3|v|3|a b|0:");
}

