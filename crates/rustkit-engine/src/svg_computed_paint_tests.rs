//! `getComputedStyle(el).fill` / `.stroke` on inline svg content (A2.1 of
//! the CSS-transitions brief; `svg_computed`). Both read empty before.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/svg_computed_paint_log.mjs`, receipt
//! `docs/diagnostics/2026-11-17/svg_computed_paint_cft148.json`: one page
//! per case, `#s` a 100x100 inline svg, each listed element's fill and
//! stroke.

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// The oracle's page for `css` and `svg`, then `id fill=.. stroke=..` for
/// each of `ids`.
fn read(css: &str, svg: &str, ids: &[&str]) -> Vec<String> {
    let page = format!(
        "<!doctype html><style>body{{margin:0}}{css}</style><svg id=s width=100 height=100>{svg}</svg>"
    );
    let server = serve_routes(vec![("/", "text/html", page)]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    ids.iter()
        .map(|id| {
            let value = engine
                .execute_script(
                    view,
                    &format!(
                        "var c=getComputedStyle(document.getElementById('{id}'));\
                         String('{id} fill='+c.fill+' stroke='+c.stroke)"
                    ),
                )
                .unwrap();
            value
                .strip_prefix("String(\"")
                .and_then(|v| v.strip_suffix("\")"))
                .unwrap_or(&value)
                .replace("\\\"", "\"")
        })
        .collect()
}

const R: &str = "<rect id=p width=10 height=10";

#[test]
fn initial_values() {
    assert_eq!(
        read("", &format!("{R} />"), &["s", "p"]),
        [
            "s fill=rgb(0, 0, 0) stroke=none",
            "p fill=rgb(0, 0, 0) stroke=none"
        ]
    );
}

#[test]
fn presentation_attributes_set_and_inherit() {
    assert_eq!(
        read(
            "",
            &format!("<g id=g fill=blue stroke=red>{R} /></g>"),
            &["g", "p"]
        ),
        [
            "g fill=rgb(0, 0, 255) stroke=rgb(255, 0, 0)",
            "p fill=rgb(0, 0, 255) stroke=rgb(255, 0, 0)"
        ]
    );
}

#[test]
fn author_rules_beat_presentation_attributes() {
    assert_eq!(
        read(
            "#p{fill:lime}",
            &format!("{R} fill=blue stroke=red />"),
            &["p"]
        ),
        ["p fill=rgb(0, 255, 0) stroke=rgb(255, 0, 0)"]
    );
}

#[test]
fn author_rules_on_the_svg_inherit() {
    assert_eq!(
        read(
            "#s{fill:blue;stroke:red}",
            &format!("<g id=g>{R} /></g>"),
            &["s", "g", "p"]
        ),
        [
            "s fill=rgb(0, 0, 255) stroke=rgb(255, 0, 0)",
            "g fill=rgb(0, 0, 255) stroke=rgb(255, 0, 0)",
            "p fill=rgb(0, 0, 255) stroke=rgb(255, 0, 0)"
        ]
    );
}

/// The style attribute against author rules, `!important` both ways, and
/// specificity among rules.
#[test]
fn the_cascade_order() {
    let own = format!("{R} style=\"fill:blue\" />");
    assert_eq!(
        read("#p{fill:lime}", &own, &["p"]),
        ["p fill=rgb(0, 0, 255) stroke=none"]
    );
    assert_eq!(
        read("#p{fill:lime!important}", &own, &["p"]),
        ["p fill=rgb(0, 255, 0) stroke=none"]
    );
    assert_eq!(
        read(
            "#p{fill:lime!important}",
            &format!("{R} style=\"fill:blue!important\" />"),
            &["p"]
        ),
        ["p fill=rgb(0, 0, 255) stroke=none"]
    );
    assert_eq!(
        read("#p{fill:blue} rect{fill:lime}", &format!("{R} />"), &["p"]),
        ["p fill=rgb(0, 0, 255) stroke=none"]
    );
}

#[test]
fn url_paints_and_none() {
    let defs = "<defs><linearGradient id=gr /></defs>";
    assert_eq!(
        read(
            "",
            &format!("{defs}{R} fill=\"url(#gr)\" stroke=\"url(#gr) red\" />"),
            &["p"]
        ),
        ["p fill=url(\"#gr\") stroke=url(\"#gr\") rgb(255, 0, 0)"]
    );
    assert_eq!(
        read(
            "#p{fill:url(#gr);stroke:url(\"#gr\") none}",
            &format!("{defs}{R} />"),
            &["p"]
        ),
        ["p fill=url(\"#gr\") stroke=url(\"#gr\") none"]
    );
    assert_eq!(
        read("#p{fill:none}", &format!("{R} stroke=none />"), &["p"]),
        ["p fill=none stroke=none"]
    );
}

/// `currentcolor` is the svg's `color`.
#[test]
fn currentcolor_is_the_svgs_color() {
    assert_eq!(
        read(
            "#s{color:blue}#p{fill:currentcolor;stroke:currentColor}",
            &format!("{R} />"),
            &["p"]
        ),
        ["p fill=rgb(0, 0, 255) stroke=rgb(0, 0, 255)"]
    );
    assert_eq!(
        read(
            "#s{color:rgb(0,128,0)}",
            &format!("{R} fill=currentColor />"),
            &["p"]
        ),
        ["p fill=rgb(0, 128, 0) stroke=none"]
    );
}

#[test]
fn colour_forms() {
    assert_eq!(
        read(
            "#p{fill:rgba(0,0,255,.5);stroke:hsl(120,100%,50%)}",
            &format!("{R} />"),
            &["p"]
        ),
        ["p fill=rgba(0, 0, 255, 0.5) stroke=rgb(0, 255, 0)"]
    );
    assert_eq!(
        read("", &format!("{R} fill=coral stroke=transparent />"), &["p"]),
        ["p fill=rgb(255, 127, 80) stroke=rgba(0, 0, 0, 0)"]
    );
}

/// An invalid attribute is ignored (the parent's value), and an author
/// `inherit` drops the attribute.
#[test]
fn invalid_attribute_and_inherit() {
    assert_eq!(
        read("", &format!("<g fill=blue>{R} fill=bogus /></g>"), &["p"]),
        ["p fill=rgb(0, 0, 255) stroke=none"]
    );
    assert_eq!(
        read(
            "#p{fill:inherit}",
            &format!("<g fill=blue>{R} fill=red /></g>"),
            &["p"]
        ),
        ["p fill=rgb(0, 0, 255) stroke=none"]
    );
}

/// Chrome 148 inherits fill / stroke from HTML ancestors. A stated gap:
/// the side cascade covers svg content only, so the svg starts from the
/// initial values.
#[test]
#[ignore = "fill / stroke on HTML ancestors of an svg are not cascaded; open row"]
fn inherited_from_an_html_ancestor() {
    assert_eq!(
        read(
            "body{fill:blue;stroke:red}",
            &format!("{R} />"),
            &["s", "p"]
        ),
        [
            "s fill=rgb(0, 0, 255) stroke=rgb(255, 0, 0)",
            "p fill=rgb(0, 0, 255) stroke=rgb(255, 0, 0)"
        ]
    );
}

// ------------------------------------------------------- R1 on #1294

/// A quoted url may hold `)`; the paint is not cut there.
#[test]
fn a_quoted_url_keeps_its_parenthesis() {
    assert_eq!(
        read(
            "",
            &format!("{R} fill='url(\"a)b\")' stroke='url(\"a)b\") red' />"),
            &["p"]
        ),
        ["p fill=url(\"a)b\") stroke=url(\"a)b\") rgb(255, 0, 0)"]
    );
}

/// A page's long `url()` chain is one invalid value (the parent's fill),
/// read without recursing once per `url()`: 100k of them used to overflow
/// the stack on the first read.
#[test]
fn a_long_url_chain_is_invalid() {
    let chain = "url(a)".repeat(100_000);
    assert_eq!(
        read(
            "",
            &format!("<g fill=blue>{R} fill=\"{chain}\" /></g>"),
            &["p"]
        ),
        ["p fill=rgb(0, 0, 255) stroke=none"]
    );
}

/// Script writes a presentation attribute inside a `display: none`
/// subtree (no relayout follows); the next read sees it (Chrome 148:
/// blue, then red, then lime).
#[test]
fn attribute_writes_in_a_hidden_subtree_are_read() {
    let server = serve_routes(vec![(
        "/",
        "text/html",
        "<!doctype html><div style=\"display:none\"><svg id=s><g id=g fill=blue>\
         <rect id=p width=10 height=10 /></g></svg></div>"
            .to_string(),
    )]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    let value = engine
        .execute_script(
            view,
            "var read=function(){return getComputedStyle(document.getElementById('p')).fill};\
             var out=[read()];document.getElementById('g').setAttribute('fill','red');out.push(read());\
             document.getElementById('p').setAttribute('fill','lime');out.push(read());String(out.join(' | '))",
        )
        .unwrap();
    assert_eq!(
        value,
        "String(\"rgb(0, 0, 255) | rgb(255, 0, 0) | rgb(0, 255, 0)\")"
    );
}

/// A page with no inline svg resolves no svg paint, however often script
/// writes and reads styles.
#[test]
fn a_page_without_svg_resolves_no_svg_paint() {
    let server = serve_routes(vec![(
        "/",
        "text/html",
        "<!doctype html><div id=d style=\"width:10px\">x</div>".to_string(),
    )]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    let before = crate::svg_computed::resolves();
    engine
        .execute_script(
            view,
            "var d=document.getElementById('d');for(var i=0;i<20;i++){\
             d.style.width=(10+i)+'px';getComputedStyle(d).width;getComputedStyle(d).fill}",
        )
        .unwrap();
    assert_eq!(crate::svg_computed::resolves() - before, 0);
}
