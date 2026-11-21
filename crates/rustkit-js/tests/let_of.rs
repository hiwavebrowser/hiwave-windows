//! `of` is an ordinary identifier name; Boa 0.20.0's parser rejected it as a
//! `let` binding (boa-dev/boa#4593, fixed upstream in 0.22.0). Minified React
//! bundles name variables `of`; github's `react-core.js` and
//! `landing-pages.js` died on it (Z2-C3 ledger).

use boa_engine::{Context, Module, Source};
use std::path::Path;

fn eval(src: &str) -> Result<String, String> {
    let mut ctx = Context::default();
    ctx.eval(Source::from_bytes(src))
        .map(|v| v.display().to_string())
        .map_err(|e| e.to_string())
}

#[test]
fn let_can_bind_the_name_of() {
    assert_eq!(eval("let of = 3; of + 1"), Ok("4".to_string()));
    assert_eq!(eval("let a = 1; let of = 2; a + of"), Ok("3".to_string()));
    assert_eq!(eval("let of=e=>e*2; of(4)"), Ok("8".to_string()));
    assert_eq!(eval("let of; of = 5; of"), Ok("5".to_string()));
    assert_eq!(eval("let [of] = [9]; of"), Ok("9".to_string()));
}

#[test]
fn a_for_loop_can_bind_the_name_of() {
    assert_eq!(eval("var n = 0; for (let of = 0; of < 3; of++) { n += of; } n"), Ok("3".to_string()));
}

#[test]
fn the_other_declaration_forms_and_for_of_still_work() {
    assert_eq!(eval("var of = 1; const o2 = 2; of + o2"), Ok("3".to_string()));
    assert_eq!(eval("var s = 0; for (let x of [1, 2, 3]) { s += x; } s"), Ok("6".to_string()));
    assert_eq!(eval("let get = 1, set = 2, from = 3, as = 4; get + set + from + as"), Ok("10".to_string()));
}

/// The construct from github's react-core.js (line 4, col 82960), reduced.
#[test]
fn the_react_core_construct_parses_as_a_module() {
    let src = "export const op = {}; op.displayName = \"SoftNavOnlyNavLink\"; let of = e => { let t, r = [1, 2]; if (r) { let t; return t; } return e; }; export { of };";
    let mut ctx = Context::default();
    Module::parse(Source::from_bytes(src).with_path(Path::new("react-core.js")), None, &mut ctx)
        .unwrap_or_else(|e| panic!("{e}"));
}
