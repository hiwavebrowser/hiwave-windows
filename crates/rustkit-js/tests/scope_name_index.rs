//! The vendored `boa_ast` looks a scope's bindings up by name through an index,
//! and declaration instantiation tests a name list through a set
//! (third_party/boa_engine/PATCHES.md, `scope-name-index`). Before, each lookup
//! scanned, so a function with many variables parsed and compiled in time that
//! grew with the square of their number:
//! YouTube's 10.9 MB `kevlar_base` bundle (one function, about 9,000
//! variables) took 5.6 s to parse and 6.4 s to compile.

use std::time::{Duration, Instant};

use boa_engine::{Context, Script, Source};

/// One function with `n` variables, each declared, assigned and read, and
/// read again from a nested function (a lookup through the outer scope).
fn bundle(n: usize) -> String {
    let mut src = String::with_capacity(n * 40);
    src.push_str("(function () {\n");
    for i in 0..n {
        src.push_str(&format!("var v{i} = {i};\n"));
    }
    src.push_str("var sum = 0;\n");
    for i in 0..n {
        src.push_str(&format!("sum += v{i};\n"));
    }
    src.push_str("return function () {\nvar t = 0;\n");
    for i in 0..n {
        src.push_str(&format!("t += v{i};\n"));
    }
    src.push_str("return t;\n};\n})();\n");
    src
}

/// One parse + compile of `src`.
fn parse_and_compile(src: &str) -> Duration {
    let mut context = Context::default();
    let start = Instant::now();
    let script = Script::parse(Source::from_bytes(src.as_bytes()), None, &mut context).expect("parse");
    script.codeblock(&mut context).expect("compile");
    start.elapsed()
}

/// The fastest of five runs of each, interleaved (small, large, small, ...)
/// so a busy moment on the host slows both sizes, not one.
fn fastest_pair(small: &str, large: &str) -> (Duration, Duration) {
    let (mut s, mut l) = (Duration::MAX, Duration::MAX);
    for _ in 0..5 {
        s = s.min(parse_and_compile(small));
        l = l.min(parse_and_compile(large));
    }
    (s, l)
}

#[test]
fn parse_and_compile_time_grows_linearly_with_a_functions_variables() {
    // Four times the variables: linear is about x4, a scan per lookup about
    // x16 (x13 measured on the scan, debug build). The bound sits between,
    // and a measurement over it is taken again, up to three times, so a
    // loaded host does not fail it; the scan was over it every time.
    let (small_src, large_src) = (bundle(5_000), bundle(20_000));
    let mut seen = Vec::new();
    for _ in 0..3 {
        let (small, large) = fastest_pair(&small_src, &large_src);
        let ratio = large.as_secs_f64() / small.as_secs_f64();
        if ratio < 6.0 {
            return;
        }
        seen.push(format!("5k {small:?}, 20k {large:?}: x{ratio:.2}"));
    }
    panic!("parse + compile grew faster than linearly: {seen:?}");
}

/// What the index answers is what the scan answered: the binding of that
/// name in the nearest scope, a redeclared `var` being one binding.
#[test]
fn names_resolve_as_before() {
    let mut context = Context::default();
    let value = context
        .eval(Source::from_bytes(
            "var a = 1; var a = 2; let b = 3;
             function f(a) { var c = a + b; { let b = 10; c += b; } return c; }
             (function () { var a = 5; return [a, f(4), typeof c, (() => a)()]; })().join()",
        ))
        .expect("eval");
    assert_eq!(value.to_string(&mut context).unwrap().to_std_string_escaped(), "5,17,undefined,5");
}
