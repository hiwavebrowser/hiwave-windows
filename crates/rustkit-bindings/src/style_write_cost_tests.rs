//! What an inline style write costs in the JavaScript copy of `CSSStyleDeclaration`, counted in VM
//! opcodes (not time). MotionMark's per-stage split (docs/diagnostics/2026-11-17/motionmark_stage_split.md)
//! puts most of a DOM subtest's frame in script, and `el.style.x = v` is the write Multiply, Leaves and
//! Design make per element per frame.
//!
//! Since NATIVE-SETPROPERTY (#1264) a page's writes take the native path (`inline_style.rs`), whose cost
//! `style_set_cost_tests` guards. This file guards the JavaScript copy, kept behind
//! `__rk.styleNative(false)`: every measurement here runs on it, so its count cannot regress unseen
//! while it is the fallback.
use super::*;

/// Bindings with the JavaScript copy of the inline style in use.
fn js_copy_bindings(html: &str) -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(html).unwrap())).unwrap();
    b.runtime
        .borrow_mut()
        .evaluate_host_bookkeeping("__rk.styleNative(false); return 0")
        .unwrap();
    b
}

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
    let b = js_copy_bindings("<html><body><div id=a style='position: absolute; left: 1px'></div></body></html>");
    b.evaluate("var el = document.getElementById('a'); var st = el.style; 'ok'").unwrap();
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

/// (name, script, budget): the measured cost with a little room. The value changes on every write (a
/// write of the same value returns early). Before (develop 02729dac): transform 11788, left 3442,
/// setProperty('opacity') 3133, backgroundColor (hsla) 9327. Most of it was `hasBadUrl` walking the
/// value one JS call per character, looking for a `url(` that almost no value has.
const OPS: &[(&str, &str, f64)] = &[
    ("st.transform = (translate, rotate)", "st.transform = 'translate(' + i + 'px, 0px) rotate(' + i + 'deg)'", 5300.0),
    ("st.left = (px)", "st.left = i + 'px'", 2700.0),
    ("st.setProperty('opacity')", "st.setProperty('opacity', '' + (i & 1))", 3100.0),
    ("st.backgroundColor = (hsla)", "st.backgroundColor = 'hsla(' + i + ', 100%, 50%, 0.5)'", 4900.0),
];

/// The measuring half, run in a child process (the VM's counter switch is read once per process).
#[test]
#[ignore]
fn style_write_cost_child() {
    for (name, op, _) in OPS {
        println!("COST|{name}|{:.1}", per_op(op));
    }
}

#[test]
fn inline_style_writes_stay_cheap() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "style_write_cost_tests::style_write_cost_child", "--ignored", "--nocapture", "--test-threads=1"])
        .env("RUSTKIT_VM_STATS", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let mut over = Vec::new();
    let mut seen = 0;
    for l in text.lines().filter_map(|l| l.find("COST|").map(|i| &l[i + 5..])) {
        let mut p = l.splitn(2, '|');
        let name = p.next().unwrap();
        let n: f64 = p.next().unwrap().parse().unwrap();
        let budget = OPS.iter().find(|o| o.0 == name).unwrap().2;
        println!("COST {name:<40} {n:>8.1} (budget {budget})");
        seen += 1;
        if n > budget {
            over.push(format!("{name}: {n:.1} > {budget}"));
        }
    }
    assert_eq!(seen, OPS.len(), "child output: {text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(over.is_empty(), "{over:?}");
}

/// The quick answer for a value with no `url(` changes no result: a bad url in any letter case is still
/// refused (the earlier value stands, CH2.E29) and a good one is kept, in both copies. (That a value
/// with no `url(` skips the walk is what the JavaScript copy's opcode counts above guard: the walk takes
/// the page's built-ins at startup since SEC-STYLE-URL, so a page cannot count it.)
#[test]
fn a_bad_url_is_still_refused_in_any_case() {
    for native in [false, true] {
        let b = js_copy_bindings("<html><body><div id=a></div></body></html>");
        b.runtime
            .borrow_mut()
            .evaluate_host_bookkeeping(&format!("__rk.styleNative({native}); return 0"))
            .unwrap();
        let got = b
            .evaluate(
                "var st = document.getElementById('a').style, out = [];
                 st.backgroundImage = 'url(ok.png)'; out.push(st.backgroundImage);
                 ['url(a b)', 'URL(a b)', 'uRl(a\"b)', 'none, url(a(b)'].forEach(function (v) { st.backgroundImage = v; out.push(st.backgroundImage); });
                 st.transform = 'rotate(3deg)'; out.push(st.transform); out.join('|')",
            )
            .unwrap();
        let got = match got {
            JsValue::String(s) => s,
            other => panic!("{other:?}"),
        };
        assert_eq!(got, "url(ok.png)|url(ok.png)|url(ok.png)|url(ok.png)|url(ok.png)|rotate(3deg)", "native {native}");
    }
}
