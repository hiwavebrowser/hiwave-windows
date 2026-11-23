//! A class constructor's `toString()` is the class's source text (`class-source-text`, see
//! third_party/boa_engine/PATCHES.md; YT-CLASS-CTOR).
//!
//! ECMA-262 ClassDefinitionEvaluation sets the constructor's [[SourceText]] to the whole
//! ClassDeclaration / ClassExpression, and `Function.prototype.toString` returns it. Boa 0.22 kept
//! no span for classes and answered `function () { [native code] }`. Polymer's legacy elements (in
//! YouTube's kevlar_base) test for native classes with `/\bclass\b/.test(class{}.toString())`; with
//! the test false they call the class constructor as a function, `N.apply(G)`, which throws
//! "class constructor cannot be invoked without 'new'" in the DOMContentLoaded handler, and ytd-app
//! never starts.
//!
//! Expected strings: Chrome for Testing 148.0.7778.216
//! (tools/parity_oracle/class_source_text_log.mjs) returns each class's source text exactly.

use rustkit_js::{JsRuntime, JsValue};

fn eval_string(rt: &mut JsRuntime, code: &str) -> String {
    match rt.evaluate_script(code).expect("evaluation succeeds") {
        JsValue::String(s) => s,
        other => panic!("expected a string, got {other:?}"),
    }
}

const DECL: &str = "class Rect extends Base {
        constructor(h, w) { super(); this.h = h; /* } */ this.w = w; }
        area() { return this.h * this.w; }
    }";
const ANON: &str = "class{}";
const NAMED: &str = "class   Named   { static x = 1; m() {} }";
const NO_CTOR: &str = "class Plain { get v() { return 1 } }";

#[test]
fn class_declaration_to_string_is_its_source_text() {
    let mut rt = JsRuntime::new().unwrap();
    rt.evaluate_script(&format!("class Base {{}}\n    {DECL}\n    {NO_CTOR}"))
        .expect("evaluation succeeds");
    assert_eq!(eval_string(&mut rt, "Rect.toString()"), DECL);
    assert_eq!(eval_string(&mut rt, "Plain.toString()"), NO_CTOR);
}

#[test]
fn class_expression_to_string_is_its_source_text() {
    let mut rt = JsRuntime::new().unwrap();
    rt.evaluate_script(&format!("var Anon = {ANON};\nvar NamedRef = {NAMED} ;"))
        .expect("evaluation succeeds");
    assert_eq!(eval_string(&mut rt, "Anon.toString()"), ANON);
    assert_eq!(eval_string(&mut rt, "NamedRef.toString()"), NAMED);
    assert_eq!(eval_string(&mut rt, "(class{}).toString()"), "class{}");
    // Nested: a class inside a function inside a class keeps its own text.
    assert_eq!(
        eval_string(
            &mut rt,
            "class Outer { m() { return class Inner extends Outer { x = 1 } } }\n\
             new Outer().m().toString()"
        ),
        "class Inner extends Outer { x = 1 }"
    );
}

#[test]
fn polymer_native_class_feature_test_is_true() {
    let mut rt = JsRuntime::new().unwrap();
    match rt
        .evaluate_script(r"/\bclass\b/.test(class{}.toString())")
        .expect("evaluation succeeds")
    {
        JsValue::Boolean(b) => assert!(b, "class{{}}.toString() must contain `class`"),
        other => panic!("expected a boolean, got {other:?}"),
    }
}

#[test]
fn class_in_eval_keeps_source_text() {
    // (`new Function(...)` keeps no source text for any function in Boa 0.22, classes or not:
    // a separate gap, not this patch's.)
    let mut rt = JsRuntime::new().unwrap();
    assert_eq!(
        eval_string(&mut rt, "eval('(class  E { })').toString()"),
        "class  E { }"
    );
}
