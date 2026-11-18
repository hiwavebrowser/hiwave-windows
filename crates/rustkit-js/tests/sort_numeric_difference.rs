//! `Array.prototype.sort` with a comparator that is exactly `return a - b` (or `b - a`) over its two plain
//! parameters compares two Numbers natively: the answers are Chrome's (Chrome for Testing 148.0.7778.216 and
//! node 22 agree on every line below), and the function is not called for such a pair. Any other comparator,
//! and any pair that is not two Numbers, is called as before.

use rustkit_js::{JsRuntime, JsValue};

fn run(src: &str) -> String {
    let mut rt = JsRuntime::new().unwrap();
    match rt.evaluate_script(src).unwrap() {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

const CASES: &str = r#"var r = [];
r.push([5,1,4,2,3,10,9].sort(function(a,b){return a-b;}).join());
r.push([5,1,4,2,3,10,9].sort(function(a,b){return b-a;}).join());
r.push([5,1,4,2,3].sort((a,b)=>a-b).join(), [5,1,4,2,3].sort((x,y)=>(y)-(x)).join());
r.push([Infinity,1,Infinity,-Infinity,NaN,3,-0,0].sort((a,b)=>a-b).map(String).join());
r.push(['10',9,'8',7,'x',1].sort((a,b)=>a-b).join());
r.push([3,1,2].toSorted((a,b)=>b-a).join());
r.push([3,,1,undefined,2].sort((a,b)=>a-b).map(String).join());
var n=0; var o=[{valueOf(){n++;return 3}},{valueOf(){n++;return 1}},{valueOf(){n++;return 2}}];
o.sort((a,b)=>a-b); r.push(o.map(Number).join(), n>0);
var calls=0; [3,1,2].sort(function(a,b){calls++;return a-b;}); r.push(calls>0);
var c2=0; [3,1,2].sort(function(a,b){return a-b+(c2++,0);}); r.push(c2>0);
r.push([3,1,2,10].sort((a,b=0)=>a-b).join(), [3,1,2,10].sort((a,b)=>b-b).join(), [3,1,2,10].sort((a,a2)=>a2-a2).join());
r.push([1n,3n,2n].sort((a,b)=>a-b<0n?-1:1).join());
r.push([3,1,2].sort((a,b)=>{return a-b;}).join(), [3,1,2].sort(function(a,b){ 'use strict'; return a-b; }).join());
r.push([0.5,0.25,2**53,2**53+2].sort((a,b)=>b-a).join());
r.join('|')
"#;

#[test]
fn numeric_difference_comparators_answer_as_chrome_does() {
    assert_eq!(
        run(CASES),
        "1,2,3,4,5,9,10|10,9,5,4,3,2,1|1,2,3,4,5|5,4,3,2,1|-Infinity,0,0,1,3,Infinity,Infinity,NaN|1,7,8,9,10,x|3,2,1|1,2,3,undefined,|1,2,3|true|true|true|1,2,3,10|3,1,2,10|3,1,2,10|1,2,3|1,2,3|1,2,3|9007199254740994,9007199254740992,0.5,0.25"
    );
}

/// Forms that look like `a - b` but are not an ordinary function: answers from Chrome for Testing 148.0.7778.216.
#[test]
fn comparators_that_are_not_plain_functions_answer_as_chrome_does() {
    let src = r#"var r = [];
function t(name, f) { try { r.push(name + '=' + f()); } catch (e) { r.push(name + '=' + e.constructor.name); } }
class C { constructor(a, b) { return a - b; } }
t('class', () => [3,1,2].sort(C).join());
t('bound', () => [3,1,2].sort(function(a,b){return a-b;}.bind(null)).join());
var calls = 0; var px = new Proxy(function(a,b){return a-b;}, { apply(t, th, args) { calls++; return Reflect.apply(t, th, args); } });
t('proxy', () => [3,1,2].sort(px).join() + ',' + (calls > 0));
var o = { m(a, b) { return a - b; } };
t('method', () => [3,1,2].sort(o.m).join());
function* g(a, b) { return a - b; }
t('generator', () => [3,1,2].sort(g).join());
async function as(a, b) { return a - b; }
t('async', () => [3,1,2].sort(as).join());
t('arrow-async', () => [3,1,2].sort(async (a, b) => a - b).join());
t('class-toSorted', () => [3,1,2].toSorted(C).join());
r.join('|')"#;
    assert_eq!(
        run(src),
        "class=TypeError|bound=1,2,3|proxy=1,2,3,true|method=1,2,3|generator=3,1,2|async=3,1,2|arrow-async=3,1,2|class-toSorted=TypeError"
    );
}

/// Counted in VM opcodes in a child process (the counter switch is read once per process).
#[test]
fn a_numeric_difference_comparator_is_not_called_per_pair() {
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args(["--exact", "sort_diff_cost_child", "--ignored", "--nocapture", "--test-threads=1"])
        .env("RUSTKIT_VM_STATS", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.contains("COST|")).unwrap_or_else(|| panic!("child: {text}{}", String::from_utf8_lossy(&out.stderr)));
    let v: Vec<u64> = line[line.find("COST|").unwrap() + 5..].split('|').map(|x| x.trim().parse().unwrap()).collect();
    let (matched, swapped, other, mixed) = (v[0], v[1], v[2], v[3]);
    assert!(matched < 100 && swapped < 100, "matched comparators still call: {matched} {swapped}");
    assert!(other > 100_000, "an unmatched comparator is called per pair: {other}");
    assert!(mixed > 100_000, "pairs that are not Numbers are called: {mixed}");
}

#[test]
#[ignore]
fn sort_diff_cost_child() {
    fn total(r: &str) -> u64 {
        r.lines().next().and_then(|l| l.split_whitespace().nth(2)).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0)
    }
    fn count(src: &str) -> u64 {
        let mut rt = JsRuntime::new().unwrap();
        rt.evaluate_script("var a = [], s = 12345; for (var i = 0; i < 4000; i++) { s = (s * 1103515245 + 12345) & 0x7fffffff; a.push(s); }").unwrap();
        let before = total(&rustkit_js::vm_stats_report());
        rt.evaluate_script(src).unwrap();
        total(&rustkit_js::vm_stats_report()) - before
    }
    let m = count("a.slice().sort(function (x, y) { return x - y; }); 1");
    let s = count("a.slice().sort((x, y) => y - x); 1");
    let o = count("a.slice().sort(function (x, y) { return x - y + 0; }); 1");
    let x = count("a.map(function (n, i) { return i % 2 ? String(n) : n; }).sort(function (x, y) { return x - y; }); 1");
    println!("COST|{m}|{s}|{o}|{x}");
}
