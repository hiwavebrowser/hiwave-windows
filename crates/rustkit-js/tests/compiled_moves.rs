//! JS-PERF2 item 6: the bytecode compiler copied a local variable into a fresh temporary before
//! every use as an operand (`Move`), and updated or assigned a `var` local through three copies.
//! On Atlas's Mac counters `Move` + `PushFromRegister` + `This` were ~32% of the opcodes executed
//! on CodeMirror's parse. This pins, deterministically (opcodes EXECUTED, from the interpreter's
//! own counters, not a timing), how many `Move`s and how many opcodes in all a typical loop costs.
//! The flag is read once per process, so this is the only test in its binary and sets it first.

use rustkit_js::{vm_stats_report, JsRuntime};

/// (total opcodes executed, `Move`s executed) according to the counters' report.
fn counts() -> (u64, u64) {
    let report = vm_stats_report();
    let total = report
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("vm stats: "))
        .and_then(|l| l.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .expect("report header");
    let moves = report
        .lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            (it.next() == Some("Move")).then(|| it.next().and_then(|n| n.parse().ok()))
        })
        .flatten()
        .unwrap_or(0);
    (total, moves)
}

#[test]
fn a_typical_loop_executes_few_moves() {
    assert_eq!(std::env::var_os("RUSTKIT_VM_STATS"), None, "run this test without the flag in the environment");
    // SAFETY: the only test in this binary; nothing else reads the environment concurrently.
    unsafe { std::env::set_var("RUSTKIT_VM_STATS", "1") };
    let mut rt = JsRuntime::new().unwrap();
    rt.evaluate_script(
        "function walk(tokens, table) {
           var counts = {}, total = 0;
           for (var j = 0; j < tokens.length; j++) {
             var t = tokens[j];
             counts[t.kind] = (counts[t.kind] || 0) + (t.to - t.from);
             if (table[t.kind & 255] === 3) { total += this.depth; }
           }
           return total;
         }
         var toks = []; for (var q = 0; q < 100; q++) toks.push({ kind: q % 4, from: q, to: q + 3 });
         var tbl = new Uint8Array(256); tbl[1] = 3;
         var self = { depth: 2, walk: walk };",
    )
    .unwrap();
    let (t0, m0) = counts();
    rt.evaluate_script("self.walk(toks, tbl);").unwrap();
    let (t1, m1) = counts();
    let (total, moves) = (t1 - t0, m1 - m0);
    println!("walk over 100 tokens: {total} opcodes, {moves} Moves");
    // Measured before the change: see the PR body. The bounds are the measured values after it.
    assert!(moves <= MOVES_BOUND, "{moves} Moves executed, expected at most {MOVES_BOUND}");
    assert!(total <= TOTAL_BOUND, "{total} opcodes executed, expected at most {TOTAL_BOUND}");
}

const MOVES_BOUND: u64 = 10;
const TOTAL_BOUND: u64 = 2350;
