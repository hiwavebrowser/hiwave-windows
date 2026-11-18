//! Host-call cost: what one DOM, CSSOM or canvas call made from page JS costs through the real
//! bindings (`DomBindings` over a parsed document, no engine, no frame). Each case runs its body `n`
//! times in one JS loop; the figure is ns per iteration, the best of `--reps` runs, with the empty
//! loop's cost subtracted. Baselines (an empty loop, a JS function call, building the same string) say
//! how much of a case is the host call itself.
//!
//! cargo run --release -p rustkit-bindings --example hostcall -- [--n 20000] [--reps 5] [--only name]

use std::rc::Rc;
use std::time::Instant;

use rustkit_bindings::DomBindings;
use rustkit_dom::Document;
use rustkit_js::JsRuntime;

const PAGE: &str = "<html><body><div id='a' style='position:absolute'></div><canvas id='c' width='300' height='150'></canvas></body></html>";

const SETUP: &str = "
var el = document.getElementById('a');
var st = el.style;
var canvas = document.getElementById('c');
var ctx = canvas.getContext('2d');
function f(x) { return x; }
";

/// (name, body); `i` is the loop index.
const CASES: &[(&str, &str)] = &[
    ("empty loop", ";"),
    ("js function call", "f(i);"),
    ("build transform string", "var t = 'translate(' + i + 'px, 0px) rotate(' + i + 'deg)';"),
    ("el.style (getter)", "el.style;"),
    ("style.transform = (cached style)", "st.transform = 'translate(' + i + 'px, 0px) rotate(' + i + 'deg)';"),
    ("el.style.transform =", "el.style.transform = 'translate(' + i + 'px, 0px) rotate(' + i + 'deg)';"),
    ("style.transform (read)", "st.transform;"),
    ("style.left = (cached style)", "st.left = i + 'px';"),
    ("el.setAttribute", "el.setAttribute('data-x', 'v' + i);"),
    ("el.setAttribute('style')", "el.setAttribute('style', 'position: absolute; transform: translate(' + i + 'px, 0px) rotate(' + i + 'deg);');"),
    ("st.setProperty('transform')", "st.setProperty('transform', 'translate(' + i + 'px, 0px) rotate(' + i + 'deg)');"),
    ("st.cssText =", "st.cssText = 'position: absolute; transform: translate(' + i + 'px, 0px) rotate(' + i + 'deg);';"),
    ("el.getAttribute('style')", "el.getAttribute('style');"),
    ("el.className =", "el.className = (i & 1) ? 'p' : 'q';"),
    ("alloc 6 objects", "var o1 = Object.create(null), o2 = Object.create(null), o3 = Object.create(null), o4 = Object.create(null), o5 = Object.create(null), o6 = Object.create(null); o1.n = i; o2.n = i;"),
    ("alloc 6 literals", "var o1 = {n: i}, o2 = {n: i}, o3 = {}, o4 = {}, o5 = {}, o6 = {};"),
    ("ctx.beginPath()", "ctx.beginPath();"),
    ("ctx.arc()", "ctx.arc(150, 75, 10 + (i & 7), 0, 6.283);"),
    ("ctx.fillStyle =", "ctx.fillStyle = (i & 1) ? 'rgb(10, 20, 30)' : 'rgb(30, 20, 10)';"),
    ("beginPath+arc+fill", "ctx.beginPath(); ctx.arc(150, 75, 10 + (i & 7), 0, 6.283); ctx.fill();"),
];

fn main() {
    let mut n = 20_000usize;
    let mut reps = 5usize;
    let mut only: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--n" => n = args.next().and_then(|v| v.parse().ok()).expect("--n N"),
            "--reps" => reps = args.next().and_then(|v| v.parse().ok()).expect("--reps N"),
            "--only" => only = args.next(),
            other => panic!("unknown argument {other}"),
        }
    }
    let bindings = DomBindings::new(JsRuntime::new().expect("runtime")).expect("bindings");
    bindings
        .set_document(Rc::new(Document::parse_html(PAGE).expect("page")))
        .expect("document");
    bindings.evaluate(SETUP).expect("setup");

    let mut empty = 0.0f64;
    for (index, (name, body)) in CASES.iter().enumerate() {
        if index != 0 && only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
            continue;
        }
        let fname = format!("__case{index}");
        bindings
            .evaluate(&format!("function {fname}(n) {{ for (var i = 0; i < n; i++) {{ {body} }} }}"))
            .expect("define");
        // One warm-up run (first-call setup, caches).
        bindings.evaluate(&format!("{fname}({})", n.min(1000))).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut best = f64::MAX;
        for _ in 0..reps {
            let t = Instant::now();
            bindings.evaluate(&format!("{fname}({n})")).unwrap_or_else(|e| panic!("{name}: {e}"));
            best = best.min(t.elapsed().as_secs_f64() * 1e9 / n as f64);
        }
        if index == 0 {
            empty = best;
        }
        println!("{:<36} {:>10.1} ns/call", name, if index == 0 { best } else { best - empty });
    }
}
