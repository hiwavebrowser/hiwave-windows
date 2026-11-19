//! What an attribute write costs, counted in VM opcodes (not time). Every `setAttribute`, `removeAttribute`,
//! `toggleAttribute`, `el.id =` and `el.className =` in a page goes through the shared write path
//! (`setData` in dom.rs); Speedometer-style pages make thousands per step, so a count cannot regress without a
//! failing test. Measured with no custom element defined and with one defined (the first
//! `customElements.define` installs the attribute-change reaction on the same path).
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
fn per_op(op: &str, with_custom_element: bool) -> f64 {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><body><div id=root><p id=a class='row r1' data-i=5>hello</p><x-w id=w title=t></x-w></div></body></html>").unwrap(),
    ))
    .unwrap();
    if with_custom_element {
        b.evaluate("customElements.define('x-w', class extends HTMLElement { static get observedAttributes() { return ['title']; } attributeChangedCallback() {} }); 'ok'").unwrap();
    }
    b.evaluate("var el = document.getElementById('a'); var cew = document.getElementById('w'); var x = 0; 'ok'").unwrap();
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

/// (name, script, budget with no custom element, budget with one defined): the measured cost with a little room.
/// Before (develop a8292900), plain / with one custom element defined: setAttribute 377 / 436, removeAttribute of
/// an absent name 339 / 400, toggleAttribute 483 / 543, `el.id =` and `el.className =` 93 / 150, a watched attribute
/// on the custom element 617, an unwatched one 456. Most of it was the steps after a write (`HOOKS.scripts`,
/// `HOOKS.frames`, `imageWritten`, `attributeChanged`) each asking a question of the node or the name that
/// almost every name answers no to.
const OPS: &[(&str, &str, f64, f64)] = &[
    ("setAttribute", "el.setAttribute('data-i', '5')", 150.0, 195.0),
    ("setAttribute (UPPER)", "el.setAttribute('DATA-I', '5')", 150.0, 195.0),
    ("removeAttribute (absent)", "el.removeAttribute('nope')", 135.0, 180.0),
    ("toggleAttribute", "el.toggleAttribute('hidden')", 250.0, 290.0),
    ("el.id = v", "el.id = 'a'", 100.0, 105.0),
    ("el.className = v", "el.className = 'row r1'", 100.0, 105.0),
    ("watched attr on the custom element", "cew.setAttribute('title', 't')", 0.0, 385.0),
    ("unwatched attr on the custom element", "cew.setAttribute('data-q', 'v')", 0.0, 195.0),
];

/// The measuring half, run in a child process (the VM's counter switch is read once per process).
#[test]
#[ignore]
fn attr_write_cost_child() {
    for ce in [false, true] {
        for (name, op, _, _) in OPS {
            if !ce && name.contains("custom element") {
                continue;
            }
            println!("COST|{}|{name}|{:.1}", u8::from(ce), per_op(op, ce));
        }
    }
}

#[test]
fn attribute_writes_stay_cheap() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "attr_write_cost_tests::attr_write_cost_child", "--ignored", "--nocapture", "--test-threads=1"])
        .env("RUSTKIT_VM_STATS", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let mut over = Vec::new();
    let mut seen = 0;
    for l in text.lines().filter_map(|l| l.find("COST|").map(|i| &l[i + 5..])) {
        let mut p = l.splitn(3, '|');
        let ce = p.next().unwrap() == "1";
        let name = p.next().unwrap();
        let n: f64 = p.next().unwrap().parse().unwrap();
        let op = OPS.iter().find(|o| o.0 == name).unwrap();
        let budget = if ce { op.3 } else { op.2 };
        println!("COST {name:<40} custom element defined: {ce:<5} {n:>8.1} (budget {budget})");
        seen += 1;
        if n > budget {
            over.push(format!("{name} (custom element {ce}): {n:.1} > {budget}"));
        }
    }
    let want = OPS.len() * 2 - OPS.iter().filter(|o| o.0.contains("custom element")).count();
    assert_eq!(seen, want, "child output: {text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(over.is_empty(), "{over:?}");
}
