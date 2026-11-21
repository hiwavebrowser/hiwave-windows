//! Class field initializers referencing outer lexical bindings threw a false-positive
//! `ReferenceError: access of uninitialized binding` in Boa 0.20 (upstream PR #4662,
//! fixed in Boa 0.22.0). github's `landing-pages.js` died on this.

use rustkit_js::{JsRuntime, JsValue};

#[test]
fn class_public_field_initializer_capturing_outer_binding() {
    let mut rt = JsRuntime::new().unwrap();
    let code = r#"
        function outer() {
            let x = 42;
            class C {
                p = x;
                m() { return this.p; }
            }
            return new C().m();
        }
        outer();
    "#;
    let res = rt.evaluate_script(code).expect("evaluation succeeds");
    match res {
        JsValue::Number(n) => assert_eq!(n, 42.0),
        other => panic!("expected Number(42), got {other:?}"),
    }
}

#[test]
fn class_private_field_initializer_capturing_outer_binding() {
    let mut rt = JsRuntime::new().unwrap();
    let code = r#"
        function outer() {
            let x = 42;
            class C {
                #p = x;
                m() { return this.#p; }
            }
            return new C().m();
        }
        outer();
    "#;
    let res = rt.evaluate_script(code).expect("evaluation succeeds");
    match res {
        JsValue::Number(n) => assert_eq!(n, 42.0),
        other => panic!("expected Number(42), got {other:?}"),
    }
}
