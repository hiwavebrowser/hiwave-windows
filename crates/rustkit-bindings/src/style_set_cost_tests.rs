//! NATIVE-SETPROPERTY: what one inline style write costs, counted in VM opcodes (not time). MotionMark's
//! Multiply writes `el.style.transform` and two more properties per tile per frame; #1261 measured one
//! `el.style.transform =` at 223 us, almost all of it CSSStyleDeclaration's setProperty interpreted in
//! JavaScript (~11,800 opcodes: the attribute parsed, the value checked, the list written back as text and
//! put through `setAttribute`). The write should be one native call: a JS call costs ~139 ns, so a write
//! of a few dozen opcodes around it is a few microseconds (the row's target: under 5 us).
use super::*;

fn grab_total(rep: &str) -> u64 {
    for l in rep.lines() {
        if let Some(r) = l.strip_prefix("vm stats: ") {
            return r.split_whitespace().next().unwrap().parse().unwrap();
        }
    }
    panic!("no vm stats line in {rep}");
}

/// Opcodes one execution of `op` costs, net of the loop with `base` in it (what builds the value).
fn per_op(op: &str, base: &str) -> f64 {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html(
            "<html><body><div id=t style='left: 1px; top: 2px; background-color: red'></div></body></html>",
        )
        .unwrap(),
    ))
    .unwrap();
    b.evaluate("var el = document.getElementById('t'), s = el.style; 'ok'").unwrap();
    let iters = 1000u64;
    let run = |body: &str| -> u64 {
        let before = grab_total(&rustkit_js::vm_stats_report());
        b.evaluate(&format!("(function(){{ for (var i = 0; i < {iters}; i++) {{ {body} }} return 'ok' }})()")).unwrap();
        grab_total(&rustkit_js::vm_stats_report()) - before
    };
    let empty = run(base);
    let cost = run(op);
    (cost as f64 - empty as f64) / iters as f64
}

/// (name, script, the loop body without the write, budget in opcodes per write). Every write changes
/// the value (`i` differs each time), so none is the unchanged-declaration no-op.
///
/// Before (develop 3e17a16f, the JavaScript CSSStyleDeclaration): 6,801 / 6,627 / 6,850 / 4,549 / 6,389.
/// Native: 153.5 / 153.8 / 191.0 / 192.0 / 327.3. What is left is the Proxy's trap, the name's
/// conversion (`backgroundColor` -> `background-color`, from a table), the value's string conversion and
/// the host call; the cheapest attribute write, `el.id = v`, is 94 with no Proxy in front
/// (`attr_write_cost_tests`). The budget is that, with room: 200 a write (~4 us of interpreter at the
/// ~19 ns an opcode #1261 measured), 400 for the remove-and-set pair.
const OPS: &[(&str, &str, &str, f64)] = &[
    ("s.transform = v", "s.transform = 'rotate(' + i + 'deg)'", "var v = 'rotate(' + i + 'deg)'", 200.0),
    ("s.backgroundColor = v", "s.backgroundColor = 'hsl(' + i + ',50%,50%)'", "var v = 'hsl(' + i + ',50%,50%)'", 200.0),
    ("s.setProperty(n, v)", "s.setProperty('transform', 'rotate(' + i + 'deg)')", "var v = 'rotate(' + i + 'deg)'", 200.0),
    ("s.setProperty(n, v, 'important')", "s.setProperty('opacity', '0.' + i, 'important')", "var v = '0.' + i", 200.0),
    ("s.removeProperty then set", "s.removeProperty('left'); s.left = i + 'px'", "var v = i + 'px'", 400.0),
];

/// The time receipt (not a gate: time depends on the machine): ns per write, native and the JavaScript
/// copy (`__rk.styleNative(false)`) on the same bindings, the best of 5 runs of 20,000, the empty loop
/// subtracted, as #1261's `hostcall` example measures.
/// `cargo test --release -p rustkit-bindings --lib style_set_time -- --ignored --nocapture`
#[test]
#[ignore]
fn style_set_time() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><body><div id='a' style='position:absolute'></div></body></html>").unwrap(),
    ))
    .unwrap();
    b.evaluate("var el = document.getElementById('a'), st = el.style; 'ok'").unwrap();
    let cases = [
        ("empty loop", ";"),
        ("st.transform = (cached style)", "st.transform = 'translate(' + i + 'px, 0px) rotate(' + i + 'deg)';"),
        ("el.style.transform =", "el.style.transform = 'translate(' + i + 'px, 0px) rotate(' + i + 'deg)';"),
        ("st.setProperty('transform')", "st.setProperty('transform', 'translate(' + i + 'px, 0px) rotate(' + i + 'deg)');"),
        ("st.left = (cached style)", "st.left = i + 'px';"),
        ("st.transform (read)", "st.transform;"),
    ];
    let n = 20_000usize;
    for native in [true, false] {
        b.runtime
            .borrow_mut()
            .evaluate_host_bookkeeping(&format!("__rk.styleNative({native}); return 0"))
            .unwrap();
        let mut empty = 0.0f64;
        for (index, (name, body)) in cases.iter().enumerate() {
            let f = format!("__time{index}");
            b.evaluate(&format!("function {f}(n) {{ for (var i = 0; i < n; i++) {{ {body} }} }} 'ok'")).unwrap();
            b.evaluate(&format!("{f}(1000)")).unwrap();
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t = std::time::Instant::now();
                b.evaluate(&format!("{f}({n})")).unwrap();
                best = best.min(t.elapsed().as_secs_f64() * 1e9 / n as f64);
            }
            if index == 0 {
                empty = best;
            }
            let ns = if index == 0 { best } else { best - empty };
            println!("TIME {:<6} {name:<32} {ns:>10.1} ns", if native { "native" } else { "js" });
        }
    }
}

/// The measuring half, run in a child process (the VM's counter switch is read once per process).
#[test]
#[ignore]
fn style_set_cost_child() {
    for (name, op, base, _) in OPS {
        println!("COST|{name}|{:.1}", per_op(op, base));
    }
}

#[test]
fn an_inline_style_write_is_one_native_call() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "style_set_cost_tests::style_set_cost_child", "--ignored", "--nocapture", "--test-threads=1"])
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
        let budget = OPS.iter().find(|o| o.0 == name).unwrap().3;
        println!("COST {name:<36} {n:>9.1} (budget {budget})");
        seen += 1;
        if n > budget {
            over.push(format!("{name}: {n:.1} > {budget}"));
        }
    }
    assert_eq!(seen, OPS.len(), "the child measured every write:\n{text}");
    assert!(over.is_empty(), "inline style writes over budget (opcodes per write): {over:?}");
}
