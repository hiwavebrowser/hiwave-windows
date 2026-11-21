//! All classic scripts of a realm share ONE global declarative environment (ES2024 §9.1.1.4 and
//! GlobalDeclarationInstantiation): a function compiled in an earlier script sees a top-level
//! `const`/`let`/`class` declared by a LATER script, and a name declared that way cannot be
//! declared again by another script. Speedometer's Perf-Dashboard dies on `RemoteAPI is not
//! defined` because a function from one `<script>` reads a `class` from the next one.
//!
//! The expected values are what the spec and Chrome 148 give; each case is one realm.

use rustkit_js::{JsRuntime, JsValue};

/// Runs each script in order in ONE realm; the result of the last, as a string. A thrown error
/// shows as `!Name` (parse errors as `!SyntaxError`).
fn run(scripts: &[&str]) -> String {
    let mut rt = JsRuntime::new().unwrap();
    let mut last = String::new();
    for s in scripts {
        last = match rt.evaluate_script(s) {
            Ok(JsValue::String(v)) => v,
            Ok(other) => format!("{other:?}"),
            Err(e) => {
                let text = e.to_string();
                let name = ["SyntaxError", "ReferenceError", "TypeError", "RangeError"]
                    .iter()
                    .find(|n| text.contains(*n))
                    .copied()
                    .unwrap_or("?");
                format!("!{name}")
            }
        };
    }
    last
}

/// `read(expr)`: a function declared in script A returning the string of `expr`, or the error name.
const READER: &str = "function read() { try { return String(x); } catch (e) { return e.name; } }";

#[test]
fn a_function_from_an_earlier_script_reads_a_const_from_a_later_one() {
    // Before the later script runs the name does not exist: ReferenceError. After: the value.
    assert_eq!(run(&[READER, "read()"]), "ReferenceError");
    assert_eq!(run(&[READER, "const x = 'c';", "read()"]), "c");
}

#[test]
fn a_function_from_an_earlier_script_reads_a_let_from_a_later_one() {
    assert_eq!(run(&[READER, "let x = 'l';", "read()"]), "l");
    // And sees later assignments.
    assert_eq!(run(&[READER, "let x = 'l';", "x = 'm';", "read()"]), "m");
}

#[test]
fn a_function_from_an_earlier_script_reads_a_class_from_a_later_one() {
    assert_eq!(run(&["function read() { return x.v(); }", "class x { static v() { return 'cv'; } }", "read()"]), "cv");
    // Speedometer's shape: the earlier script uses the class when called, the later one declares it.
    assert_eq!(
        run(&["function use() { return new RemoteAPI().v; }", "class RemoteAPI { v = 7; }", "String(use())"]),
        "7"
    );
}

#[test]
fn an_earlier_script_cannot_use_a_later_declaration_before_it_runs() {
    // The binding exists only once the later script's declaration instantiation has run, so the
    // earlier script's own top-level code gets a ReferenceError, not undefined and not a value.
    assert_eq!(run(&["var r1 = (function () { try { return String(x); } catch (e) { return e.name; } })();", "const x = 1;", "r1"]), "ReferenceError");
}

#[test]
fn a_temporal_dead_zone_read_inside_the_declaring_script_is_a_reference_error() {
    assert_eq!(run(&[READER, "var seen = read(); const x = 'late'; seen"]), "ReferenceError");
    assert_eq!(run(&[READER, "var seen = read(); const x = 'late'; read()"]), "late");
}

#[test]
fn redeclaring_a_lexical_name_in_a_later_script_is_a_syntax_error() {
    assert_eq!(run(&["const a = 1;", "const a = 2;"]), "!SyntaxError");
    assert_eq!(run(&["let a = 1;", "let a = 2;"]), "!SyntaxError");
    assert_eq!(run(&["class a {}", "class a {}"]), "!SyntaxError");
    assert_eq!(run(&["let a = 1;", "var a = 2;"]), "!SyntaxError");
    assert_eq!(run(&["let a = 1;", "function a() {}"]), "!SyntaxError");
    assert_eq!(run(&["var a = 1;", "let a = 2;"]), "!SyntaxError");
    // The failed script declared nothing: the original binding survives.
    assert_eq!(run(&["let a = 1;", "let a = 2;", "String(a)"]), "1");
}

#[test]
fn a_global_lexical_binding_is_not_a_property_of_the_global_object() {
    assert_eq!(run(&["const a = 1;", "String(globalThis.a)"]), "undefined");
    assert_eq!(run(&["var b = 1;", "String(globalThis.b)"]), "1");
}

#[test]
fn eval_and_new_function_see_a_later_scripts_lexical_declaration() {
    assert_eq!(run(&["var g = new Function('return String(x)');", "const x = 'nf';", "g()"]), "nf");
    assert_eq!(run(&["function r() { return eval('String(x)'); }", "let x = 'ev';", "r()"]), "ev");
    assert_eq!(run(&["function r() { return (0, eval)('x.v()'); }", "class x { static v() { return 'ie'; } }", "r()"]), "ie");
    // A const declared by an indirect eval is local to that eval, not global.
    assert_eq!(run(&["(0, eval)('const z = 1');", "typeof z"]), "undefined");
    // Redeclaring a global lexical name through a direct or indirect var eval is a SyntaxError.
    assert_eq!(run(&["let q = 1;", "try { (0, eval)('var q = 2'); 'no' } catch (e) { e.name }"]), "SyntaxError");
}

#[test]
fn an_earlier_function_can_assign_typeof_and_update_a_later_lexical_binding() {
    let setter = "function set(v) { x = v; } function ty() { return typeof x; } function inc() { return ++x; }";
    assert_eq!(run(&[setter, "let x = 1;", "set(5); String(x)"]), "5");
    assert_eq!(run(&[setter, "let x = 1;", "String(inc())"]), "2");
    assert_eq!(run(&[setter, "String(ty())"]), "undefined");
    assert_eq!(run(&[setter, "let x = 1;", "ty()"]), "number");
    // A const stays const: assignment from the earlier script's function is a TypeError.
    assert_eq!(
        run(&["function set() { try { x = 2; return 'no'; } catch (e) { return e.name; } }", "const x = 1;", "set()"]),
        "TypeError"
    );
    // Before the later script declares it, a sloppy assignment still creates a global property.
    assert_eq!(run(&[setter, "set(3); String(globalThis.x)"]), "3");
}

/// Assigning a later script's lexical binding BEFORE its declaration has run (R1 on #1101 asked for
/// ReferenceError for a `const`). Chrome for Testing 148.0.7778.216, measured with two script tags:
/// a `const` answers TypeError (the constness is checked first), a `let` answers ReferenceError, and a
/// read or `typeof` of either answers ReferenceError. Ours matches each.
#[test]
fn assigning_or_reading_a_later_lexical_binding_before_it_is_initialised_matches_chrome() {
    assert_eq!(run(&["function set() { x = 2 }", "var r; try { set() } catch (e) { r = e.name } const x = 1;", "r"]), "TypeError");
    assert_eq!(run(&["function set() { y = 2 }", "var r; try { set() } catch (e) { r = e.name } let y = 1;", "r"]), "ReferenceError");
    assert_eq!(run(&["function set() { try { z = 2 } catch (e) { return e.name } }", "const z = 1;", "set()"]), "TypeError");
    assert_eq!(run(&["function get() { return w }", "var r; try { get() } catch (e) { r = e.name } const w = 1;", "r"]), "ReferenceError");
    assert_eq!(run(&["function ty() { return typeof v }", "var r; try { ty() } catch (e) { r = e.name } const v = 1;", "r"]), "ReferenceError");
}

#[test]
fn typeof_an_undeclared_global_is_still_undefined() {
    assert_eq!(run(&["String(typeof neverDeclaredAnywhere)"]), "undefined");
    // After the declaration has run, `typeof` of the late binding is its value's type.
    assert_eq!(run(&["function ty() { return typeof late }", "const late = 1;", "ty()"]), "number");
}
