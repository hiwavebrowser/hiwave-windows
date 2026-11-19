//! A declaration list may continue on the next line with a leading comma:
//! `const a = 1\n, b = 2`. Speedometer 3's Lit bundles are written that way; the engine threw
//! `SyntaxError: unexpected token ','` (STR-5, found by the STRESS1 first run).

use boa_engine::{Context, Source};

fn eval(src: &str) -> Result<String, String> {
    let mut ctx = Context::default();
    ctx.eval(Source::from_bytes(src))
        .map(|v| v.display().to_string())
        .map_err(|e| e.to_string())
}

#[test]
fn a_const_list_continues_after_a_newline_and_a_leading_comma() {
    assert_eq!(eval("const a = 1\n, b = 2; a + b"), Ok("3".to_string()));
    assert_eq!(eval("const a = 1\r\n  , b = 2\r\n  , c = 3; a + b + c"), Ok("6".to_string()));
}

#[test]
fn let_lists_continue_and_var_is_the_control_that_always_did() {
    assert_eq!(eval("let a = 1\n, b = 2; a + b"), Ok("3".to_string()));
    assert_eq!(eval("var a = 1\n, b = 2; a + b"), Ok("3".to_string()));
}

#[test]
fn an_initialiser_that_ends_a_line_still_continues() {
    assert_eq!(eval("const f = () => 1\n, g = () => 2; f() + g()"), Ok("3".to_string()));
    assert_eq!(eval("const o = {x: 1}\n, p = [2]; o.x + p[0]"), Ok("3".to_string()));
}

#[test]
fn a_leading_comma_continues_a_for_loop_head() {
    assert_eq!(eval("var s = 0; for (let i = 0
, n = 3; i < n; i++) { s += i } s"), Ok("3".to_string()));
    assert_eq!(eval("var k = 0; for (const a = 1
, b = 2;;) { k = a + b; break } k"), Ok("3".to_string()));
}

#[test]
fn a_newline_without_a_comma_still_ends_the_declaration() {
    // ASI is intact: two statements, not a list.
    assert_eq!(eval("var b; let a = 1
b = 2; a + b"), Ok("3".to_string()));
    assert_eq!(eval("let a = 1
; a"), Ok("1".to_string()));
}

#[test]
fn a_let_list_continues_across_crlf() {
    assert_eq!(eval("let a = 1
  , b = 2
  , c = 3; a + b + c"), Ok("6".to_string()));
}

