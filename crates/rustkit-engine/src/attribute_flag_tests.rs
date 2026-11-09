//! Attribute selector case flags (CH2.E18, Selectors 4 §6.3).
//!
//! `[a="v" i]` compares the value ASCII case-insensitively. The matcher
//! kept the flag as part of the value (`"v" i`), so no flagged selector
//! ever matched. The page is `tools/parity_oracle/attribute_flag_page.html`;
//! every `nav` is `display: none` until its own rule shows it. The expected
//! answers are what `attribute_flag_log.mjs` reads from Chromium 143:
//! `i`/`I` fold ASCII only (`é` is not `É`); `s` and any other flag make
//! the selector invalid, so its rule is dropped and a script query throws.

use super::*;

const PAGE: &str = include_str!("../../../tools/parity_oracle/attribute_flag_page.html");

fn shown(html: &str) -> Vec<String> {
    let document = Rc::new(Document::parse_html(html).expect("html"));
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let layout = engine.build_layout_from_document(&document, &[]);
    fn texts(b: &LayoutBox, out: &mut Vec<String>) {
        if let BoxType::Text(t) = &b.box_type {
            if !t.trim().is_empty() {
                out.push(t.trim().to_string());
            }
        }
        for c in &b.children {
            texts(c, out);
        }
    }
    let mut out = Vec::new();
    texts(&layout, &mut out);
    out.dedup();
    out
}

#[test]
fn a_flagged_attribute_selector_matches_as_chromium_does() {
    assert_eq!(
        shown(PAGE).join(" "),
        "a1 b1 c1 d1 e1 f1 g1 k1 l1 m1 n1 o1 p1 q1 u1 v1"
    );
}

#[test]
fn the_flag_splits_from_the_value_and_folds_only_with_i() {
    for (test, value, passes) in [
        ("data-x=\"Foo\" i", "fOO", true),
        ("data-x=\"Foo\" I", "FOO", true),
        ("data-x=\"Foo\"i", "foo", true),
        ("data-x='Foo' i", "foo", true),
        ("data-x=Foo i", "foo", true),
        ("data-x = \"Foo\"  i ", "foo", true),
        ("data-x=\"Foo\" i", "bar", false),
        ("data-x~=\"B\" i", "a b", true),
        ("data-x|=\"EN\" i", "en-us", true),
        ("data-x^=\"AB\" i", "abc", true),
        ("data-x$=\"YZ\" i", "xyz", true),
        ("data-x*=\"MID\" i", "amidb", true),
        ("data-x=\"é\" i", "É", false),
        ("data-x=\"Foo\"", "foo", false),
        ("data-x=\"Foo\" s", "Foo", false),
        ("data-x=\"Foo\" x", "Foo", false),
        // An operator inside the quoted value is part of the value.
        ("data-x=\"a~=b\"", "a~=b", true),
        ("data-x=\"a^=b\" i", "A^=B", true),
    ] {
        let attributes: HashMap<String, String> = [("data-x".to_string(), value.to_string())].into();
        assert_eq!(
            SelectorMatcher.match_attribute_selector(test, &attributes),
            passes,
            "[{test}] on data-x={value:?}"
        );
        assert_eq!(SelectorMatcher::attr_selector_name(test), "data-x", "[{test}]");
    }
    assert!(SelectorMatcher::selector_list_is_valid("[data-x=\"a\" i]"));
    assert!(SelectorMatcher::selector_list_is_valid("[data-x=a I], b"));
    assert!(!SelectorMatcher::selector_list_is_valid("[data-x=\"a\" s]"));
    assert!(!SelectorMatcher::selector_list_is_valid("b, [data-x=\"a\" x]"));
    // A forgiving list drops only the bad member.
    assert!(SelectorMatcher::selector_list_is_valid(":is([data-x=\"a\" s], b)"));
}

#[cfg(feature = "headless")]
mod script {
    use super::*;

    fn eval(engine: &mut Engine, view: EngineViewId, script: &str) -> String {
        let out = engine
            .execute_script(view, &format!("String(eval({script:?}))"))
            .unwrap();
        out.strip_prefix("String(\"")
            .and_then(|s| s.strip_suffix("\")"))
            .unwrap_or_else(|| panic!("{script} evaluated to {out}"))
            .to_string()
    }

    #[test]
    fn script_queries_take_the_flag_as_chromium_does() {
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
            .expect("view");
        engine.load_html(view, PAGE).expect("load");
        let mut js = |s: &str| eval(&mut engine, view, s);
        let q = |s: &str| {
            format!(
                "(() => {{ try {{ return [...document.querySelectorAll({s:?})]\
                 .map(n => n.textContent).join(' '); }} catch (e) {{ return 'throws ' + e.name; }} }})()"
            )
        };
        // Chromium 143's answers (attribute_flag_log.mjs).
        assert_eq!(js(&q("[data-a=\"FOO\" i]")), "a1");
        assert_eq!(js(&q("[data-h=\"foo\" s]")), "throws SyntaxError");
        assert_eq!(js(&q("[data-b=Foo i]")), "b1");
        assert_eq!(js(&q("[data-r=\"foo\" x]")), "throws SyntaxError");
        assert_eq!(
            js("document.querySelector('[data-k]').matches('[data-k=\"foo\" i]')"),
            "true"
        );
        assert_eq!(
            js("document.querySelector('[data-m]').firstElementChild.closest('[data-m=\"open\" i]').tagName"),
            "DIV"
        );
    }
}
