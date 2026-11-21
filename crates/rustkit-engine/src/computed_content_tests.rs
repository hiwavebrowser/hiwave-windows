//! `getComputedStyle(element).content`. `computed_style_lines` had no
//! `content` row, so script read an empty string. netflix's hero form picks
//! its layout from it: the wrapper says `content: '1'`, and `content: '2'`
//! from its medium breakpoint up, and `useSplitForm` keeps the form stacked
//! (`data-issplitform="true"`) while the digits read `1`, or nothing at all.
//! Chrome 148 at 1280x800 reads `"2"` and puts the field and button in one
//! row (Pete's hand test 10-27, HT27 N2).

use super::*;

/// The `content` CSSOM value for a style, via the same `computed_style_lines`
/// row script reads. Portable: no headless view, no GPU.
fn content_cssom(style: ComputedStyle) -> String {
    let box_ = LayoutBox::new(rustkit_layout::BoxType::Block, style);
    for line in computed_style_lines(&box_).lines() {
        if let Some(v) = line.strip_prefix("content\t") {
            return v.to_string();
        }
    }
    panic!("computed_style_lines dropped the content row");
}

/// #821: the CSSOM serialize for a cascaded string — quotes, escapes, and
/// `none`/`normal` both reading as `normal`. The macOS+headless pin below
/// covers the `@media` cascade end-to-end; Linux CI never saw that module's
/// only test, so the serialize contract lived untested on every default
/// `cargo test -p rustkit-engine --lib`.
#[test]
fn computed_content_serializes_like_cssom() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let apply = |value: &str| {
        let mut style = ComputedStyle::default();
        engine
            .lctx
            .apply_style_property(&mut style, "content", value);
        content_cssom(style)
    };

    assert_eq!(
        content_cssom(ComputedStyle::default()),
        "normal",
        "unset content reads normal, never empty"
    );
    assert_eq!(apply("none"), "normal");
    assert_eq!(apply("normal"), "normal");
    assert_eq!(apply("'1'"), "\"1\"");
    assert_eq!(apply("'2'"), "\"2\"");
    assert_eq!(apply(r#""a\"b""#), "\"a\\\"b\"");
    // A resolved `\a` serialises back as `\a ` (CSSOM), never as a raw
    // line break that would split the tab-separated computed-style row.
    assert_eq!(apply(r#""a\a b""#), "\"a\\a b\"");
    assert_eq!(css_string("a\nb"), "\"a\\a b\"");
    assert_eq!(css_string("a\\b"), "\"a\\\\b\"");
    assert_eq!(css_string("a\"b"), "\"a\\\"b\"");
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn computed_content_is_the_cascaded_string_or_normal() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 1280, 800))
        .expect("headless view");
    engine
        .load_html(
            id,
            r#"<!doctype html><style>#w{content:'1'} @media (min-width: 600px){#w{content:'2'}}
               #q{content:"a\"b"} #n{content:none} #l{content:"a\a b"}</style>
               <body><div id=w><form id=f>x</form></div><div id=q>x</div><div id=n>x</div><div id=d>x</div><div id=l>x</div></body>"#,
        )
        .expect("load_html");
    let value = engine
        .execute_script(
            id,
            "['w', 'f', 'q', 'n', 'd', 'l'].map(function (i) { return i + '=' + \
             getComputedStyle(document.getElementById(i)).content; }).join(' ')",
        )
        .expect("script");
    // Chrome 148.0.7778.216, same page; a resolved `\a` serialises back as
    // `\a ` (CSSOM), never as a raw line break that would split the row.
    let chrome = r#"w="2" f=normal q="a\"b" n=normal d=normal l="a\a b""#;
    assert_eq!(value, format!("String({chrome:?})"));
}
