//! `getComputedStyle(element, pseudo)`. The second argument was ignored, so
//! the breakpoint idiom `getComputedStyle(body, "::before").content` read the
//! host element's row. Expected values are Chromium 143.0.7499.4's answers on
//! this page (`trench/z/tools/gcs1/gcs1_probe.mjs`) unless a test says it pins
//! a gap.

use super::*;

/// A `content` with no string (`attr()`, `counter()`, `url()`, a quote
/// keyword) replaces a string from a lower-precedence rule. The engine
/// generates nothing for these yet, so the stale string must not paint.
#[test]
fn content_with_no_string_resets_an_earlier_string() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    for value in ["attr(data-x)", "counter(c)", "url(x.gif)", "open-quote"] {
        let mut style = ComputedStyle::default();
        engine.lctx.apply_style_property(&mut style, "content", "'lower'");
        engine.lctx.apply_style_property(&mut style, "content", value);
        assert_eq!(style.content, None, "{value}");
    }
    // An invalid value is dropped; the earlier declaration stands.
    let mut style = ComputedStyle::default();
    engine.lctx.apply_style_property(&mut style, "content", "'lower'");
    engine.lctx.apply_style_property(&mut style, "content", "5px");
    assert_eq!(style.content.as_deref(), Some("lower"));
}

#[cfg(all(target_os = "macos", feature = "headless"))]
mod page {
    use super::*;

    const PAGE: &str = r#"<!doctype html><style>
        #hidden::before{content:"md";display:none}
        #shown::before{content:"bf";color:rgb(1,2,3);position:absolute;width:10px}
        #shown::after{content:"af" "x"}
        #shown{color:rgb(9,9,9)}
        #own{content:"z";color:rgb(4,5,6)}
        #none::before{content:none;color:rgb(7,8,9)}
        #toggle.on::before{content:"t"}
        </style><body>
        <div id=hidden>x</div><div id=shown>x</div><div id=own>x</div><div id=none>x</div>
        <div id=plain>x</div><div id=toggle class=on>x</div></body>"#;

    fn loaded() -> (Engine, EngineViewId) {
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine.create_headless_view(Bounds::new(0, 0, 1280, 800)).expect("headless view");
        engine.load_html(id, PAGE).expect("load_html");
        (engine, id)
    }

    fn read(engine: &mut Engine, id: EngineViewId, element: &str, pseudo: &str) -> String {
        let script = format!(
            "var c = getComputedStyle(document.getElementById('{element}'), {pseudo}); \
             [c.content, c.display, c.color, c.position, c.width].join('|')"
        );
        let value = engine.execute_script(id, &script).expect("script");
        value.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&value).replace("\\\"", "\"")
    }

    #[test]
    fn a_pseudo_with_display_none_still_answers() {
        let (mut e, id) = loaded();
        let chrome = "\"md\"|none|rgb(0, 0, 0)|static|auto";
        assert_eq!(read(&mut e, id, "hidden", "'::before'"), chrome);
        assert_eq!(read(&mut e, id, "hidden", "':before'"), chrome);
    }

    #[test]
    fn a_pseudo_answers_its_own_style_and_inherits_the_rest() {
        let (mut e, id) = loaded();
        let before = "\"bf\"|block|rgb(1, 2, 3)|absolute|10px";
        assert_eq!(read(&mut e, id, "shown", "'::before'"), before);
        assert_eq!(read(&mut e, id, "shown", "'::BEFORE'"), before);
        assert_eq!(read(&mut e, id, "shown", "'before'"), before);
        assert_eq!(read(&mut e, id, "shown", "'::after'"), "\"afx\"|inline|rgb(9, 9, 9)|static|auto");
        assert_eq!(read(&mut e, id, "shown", "':after'"), "\"afx\"|inline|rgb(9, 9, 9)|static|auto");
        // GAP, not Chromium: it reads the rule's rgb(7, 8, 9). A pseudo with
        // no `content` string keeps no cascaded style, so only what it
        // inherits is known.
        assert_eq!(read(&mut e, id, "none", "'::before'"), "none|inline|rgb(0, 0, 0)|static|auto");
    }

    #[test]
    fn no_pseudo_rule_reads_none_with_what_it_inherits() {
        let (mut e, id) = loaded();
        assert_eq!(read(&mut e, id, "plain", "'::before'"), "none|inline|rgb(0, 0, 0)|static|auto");
        assert_eq!(read(&mut e, id, "hidden", "'::after'"), "none|inline|rgb(0, 0, 0)|static|auto");
        // The element's own `content` is not its pseudo's.
        assert_eq!(read(&mut e, id, "own", "'::before'"), "none|inline|rgb(4, 5, 6)|static|auto");
    }

    #[test]
    fn no_second_argument_reads_the_element_and_lists_no_pseudo_rows() {
        let (mut e, id) = loaded();
        let element = "normal|block|rgb(9, 9, 9)|static|1264px";
        for arg in ["undefined", "null", "''"] {
            assert_eq!(read(&mut e, id, "shown", arg), element, "{arg}");
        }
        assert_eq!(read(&mut e, id, "own", "undefined"), "\"z\"|block|rgb(4, 5, 6)|static|1264px");
        let listed = e
            .execute_script(
                id,
                "Array.from(getComputedStyle(document.getElementById('shown'))).some(function (n) { \
                 return n.charAt(0) === ':'; }) + '|' + \
                 getComputedStyle(document.getElementById('shown')).cssText.indexOf('::')",
            )
            .expect("script");
        assert_eq!(listed, "String(\"false|-1\")");
    }

    #[test]
    fn an_unknown_pseudo_is_an_empty_style_and_first_line_reads_normal() {
        let (mut e, id) = loaded();
        assert_eq!(read(&mut e, id, "shown", "'::foo'"), "||||");
        assert_eq!(read(&mut e, id, "shown", "'::first-line'"), "normal|inline|rgb(9, 9, 9)|static|auto");
    }

    #[test]
    fn a_pseudo_read_follows_a_script_write() {
        let (mut e, id) = loaded();
        assert_eq!(read(&mut e, id, "toggle", "'::before'"), "\"t\"|inline|rgb(0, 0, 0)|static|auto");
        e.execute_script(id, "document.getElementById('toggle').className = ''").expect("script");
        assert_eq!(read(&mut e, id, "toggle", "'::before'"), "none|inline|rgb(0, 0, 0)|static|auto");
    }
}
