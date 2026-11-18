//! A runaway recursion in page script must be a catchable `RangeError`, not the end of the script
//! (STR-10). The stress suites probe stack depth and carry on.

use rustkit_js::{JsRuntime, JsValue};

fn run(src: &str) -> Result<JsValue, String> {
    let mut rt = JsRuntime::new().map_err(|e| e.to_string())?;
    rt.evaluate_script(src).map_err(|e| e.to_string().lines().next().unwrap_or("").to_string())
}

fn text(v: Result<JsValue, String>) -> String {
    match v {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => n.to_string(),
        other => format!("{other:?}"),
    }
}

#[test]
fn a_runaway_recursion_is_a_catchable_range_error() {
    let src = "function f(){ return f() + 1; } var r; try { f(); r = 'no error'; } catch (e) { r = e instanceof RangeError ? 'RangeError' : String(e); } r";
    assert_eq!(text(run(src)), "RangeError");
}

#[test]
fn the_script_goes_on_after_the_overflow_is_caught() {
    let src = "function f(){ return f() + 1; } try { f(); } catch (e) {} var n = 0; for (var i = 0; i < 3; i++) n += i; n";
    assert_eq!(text(run(src)), "3");
}

#[test]
fn a_method_and_a_closure_cycle_overflow_the_same_way() {
    let src = "var o = { m() { return this.m(); } }; var a = () => b(); var b = () => a(); var out = []; try { o.m(); } catch (e) { out.push(e.name); } try { a(); } catch (e) { out.push(e.name); } out.join()";
    assert_eq!(text(run(src)), "RangeError,RangeError");
}

#[test]
fn an_uncaught_overflow_still_ends_the_script_with_an_error() {
    assert!(run("function f(){ return f() + 1; } f();").is_err());
}
