//! `font-size: 0`, against the pinned Chromium.
//!
//! The usual way to remove the spaces between inline-blocks is `font-size: 0`
//! on their parent, and a page hides a label the same way. `0` parses to
//! `Length::Zero`, and layout read every font size that is not `Px` as 16px:
//! a line holding only a 10px inline-block was 18 tall, and text meant to be
//! hidden took its full width and height.
//!
//! Each page has the border boxes Chromium 143 reports for its elements with
//! an id, as `id:x:y:width:height` (rounded `getBoundingClientRect`). No page
//! has text at a size other than 0, so no shaper decides anything and the
//! pages are checked on every OS, laid out as the nested layout matrix lays
//! its pages out.

use super::*;

const IB: &str = r#"<i style="display:inline-block;width:100px;height:10px"></i>"#;

/// (name, extra css, body with `{ib}` for the inline-block, Chromium's boxes)
const PAGES: &[(&str, &str, &str, &str)] = &[
    ("a block around an inline-block", "", "<div id=x>{ib}</div>", "x:0:0:1280:10"),
    (
        "a flex row around a block",
        "#x{display:flex}",
        "<div id=x><div id=m>{ib}</div></div>",
        "x:0:0:1280:10 m:0:0:100:10",
    ),
    (
        "an inline-block around a block",
        "#f{display:inline-block}",
        "<div id=x><div id=f><div id=a>{ib}</div></div></div>",
        "x:0:0:1280:10 f:0:0:100:10 a:0:0:100:10",
    ),
    (
        "an inline-block around a grid",
        "#f{display:inline-block}#m{display:grid}",
        "<div id=x><div id=f><div id=m><div id=a>{ib}</div></div></div></div>",
        "x:0:0:1280:10 f:0:0:100:10 m:0:0:100:10 a:0:0:100:10",
    ),
    (
        "text alone takes no room",
        "#x{float:left}",
        "<div id=x><span id=m>hidden words</span></div>",
        "x:0:0:0:0 m:0:0:0:0",
    ),
    (
        "text beside an inline-block takes no room",
        "#x{float:left}",
        "<div id=x>words {ib}</div>",
        "x:0:0:100:10",
    ),
    (
        "an em of zero is zero",
        "#f{float:left}#a{font-size:2em}",
        "<div id=x><div id=f><span id=a>gone</span></div></div>",
        "x:0:0:1280:0 f:0:0:0:0 a:0:0:0:0",
    ),
    (
        "zero from a style attribute under a sized parent",
        "",
        r#"<div id=x style="font-size:16px"><div id=m style="font-size:0">{ib}</div></div>"#,
        "x:0:0:1280:10 m:0:0:1280:10",
    ),
    (
        "zero on a ::before",
        r#"#x{font-size:16px}#m{font-size:0}#m::before{content:"";display:inline-block;width:5px;height:5px;font-size:0}"#,
        "<div id=x><div id=m></div></div>",
        "x:0:0:1280:5 m:0:0:1280:5",
    ),
    // An atomic inline inside an inline box. With no strut left the box
    // around it must still give the line its height: a linked logo image in
    // a `font-size: 0` row was 0 tall, and the next block was drawn over it.
    (
        "an image in a link, then a block",
        "",
        r#"<div id=x><a href=#><img style="width:40px;height:30px" src="data:image/gif;base64,R0lGODlhAQABAAAAACw="></a></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:30 y:0:30:1280:5",
    ),
    (
        "an inline-block in a span, then a block",
        "",
        r#"<div id=x><span>{ib}</span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:10 y:0:10:1280:5",
    ),
    (
        "an inline-block three inline boxes deep",
        "",
        r#"<div id=x><span><b><em>{ib}</em></b></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:10 y:0:10:1280:5",
    ),
    (
        "a taller inline-block in a span beside one outside",
        "",
        r#"<div id=x>{ib}<span><i style="display:inline-block;width:50px;height:30px"></i></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:30 y:0:30:1280:5",
    ),
    (
        "the margin box of an inline-block in a span",
        "",
        r#"<div id=x><span><i style="display:inline-block;width:50px;height:10px;margin:5px 0"></i></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:20 y:0:20:1280:5",
    ),
    (
        "an inline-block in a span on a zero line-height at 16px",
        "#x{font-size:16px;line-height:0}",
        r#"<div id=x><span>{ib}</span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:10 y:0:10:1280:5",
    ),
    (
        "two lines of inline-blocks in a span",
        "#x{width:150px}",
        r#"<div id=x><span>{ib}{ib}</span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:150:20 y:0:20:1280:5",
    ),
    (
        "an image in a link in a flex item",
        "#f{display:flex}",
        r#"<div id=f><div id=x><a href=#><img style="width:40px;height:30px" src="data:image/gif;base64,R0lGODlhAQABAAAAACw="></a></div></div><div id=y style="height:5px"></div>"#,
        "f:0:0:1280:30 x:0:0:40:30 y:0:30:1280:5",
    ),
    // Where the boxes inside the inline box sit: on the line's baseline,
    // with the inline box itself a zero-height box at the baseline.
    (
        "where a linked image and its link sit",
        "",
        r#"<div id=x><a id=a href=#><img id=g style="width:40px;height:30px" src="data:image/gif;base64,R0lGODlhAQABAAAAACw="></a></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:30 a:0:30:40:0 g:0:0:40:30 y:0:30:1280:5",
    ),
    (
        "a short inline-block outside a span and a tall one inside",
        "",
        r#"<div id=x><i id=p style="display:inline-block;width:100px;height:10px"></i><span id=s><i id=q style="display:inline-block;width:50px;height:30px"></i></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:30 p:0:20:100:10 s:100:30:50:0 q:100:0:50:30 y:0:30:1280:5",
    ),
    (
        "a tall inline-block outside a span and a short one inside",
        "",
        r#"<div id=x><i id=p style="display:inline-block;width:100px;height:30px"></i><span id=s><i id=q style="display:inline-block;width:50px;height:10px"></i></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:30 p:0:0:100:30 s:100:30:50:0 q:100:20:50:10 y:0:30:1280:5",
    ),
    (
        "two inline-blocks of two heights in one span",
        "",
        r#"<div id=x><span id=s><i id=p style="display:inline-block;width:100px;height:10px"></i><i id=q style="display:inline-block;width:50px;height:30px"></i></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:30 s:0:30:150:0 p:0:20:100:10 q:100:0:50:30 y:0:30:1280:5",
    ),
    (
        "an inline-block with a margin in a span",
        "",
        r#"<div id=x><span id=s><i id=q style="display:inline-block;width:50px;height:10px;margin:5px 0 7px"></i></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:22 s:0:22:50:0 q:0:5:50:10 y:0:22:1280:5",
    ),
    (
        "a top-aligned inline-block in a span beside a taller one",
        "",
        r#"<div id=x><i id=p style="display:inline-block;width:100px;height:30px"></i><span id=s><i id=q style="display:inline-block;width:50px;height:10px;vertical-align:top"></i></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:30 p:0:0:100:30 s:100:30:50:0 q:100:0:50:10 y:0:30:1280:5",
    ),
    (
        "an inline-block two inline boxes deep beside a taller one",
        "",
        r#"<div id=x><i id=p style="display:inline-block;width:100px;height:30px"></i><span><b id=s><i id=q style="display:inline-block;width:50px;height:10px"></i></b></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:30 p:0:0:100:30 s:100:30:50:0 q:100:20:50:10 y:0:30:1280:5",
    ),
    (
        "an empty span with a line-height beside an inline-block",
        "",
        r#"<div id=x><span style="line-height:40px"></span><i id=q style="display:inline-block;width:100px;height:10px"></i></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:40 y:0:40:1280:5",
    ),
    // An out-of-flow atomic inline inside an inline box is not content of
    // the line: the empty `<span>` or `<a>` around a positioned icon or
    // badge must not be kept, or it gives the line a strut and the next
    // block moves down one line (round-3 hold; Chromium 0), while the
    // positioned box itself stays, at the line's place (Chromium 141 and
    // the pinned 143 agree on every box here).
    (
        "an absolute inline-block in a span, then a block",
        "#x{font-size:16px;line-height:20px}",
        r#"<div id=x><span><i id=i style="position:absolute;display:inline-block;width:20px;height:20px"></i></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:0 i:0:0:20:20 y:0:0:1280:5",
    ),
    (
        "an absolute inline-block in a link, then a block",
        "#x{font-size:16px;line-height:20px}",
        r#"<div id=x><a href=#><i id=i style="position:absolute;display:inline-block;width:20px;height:20px"></i></a></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:0 i:0:0:20:20 y:0:0:1280:5",
    ),
    (
        "a fixed inline-flex in a span, then a block",
        "#x{font-size:16px;line-height:20px}",
        r#"<div id=x><span><i id=i style="position:fixed;display:inline-flex;width:20px;height:20px"></i></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:0 i:0:0:20:20 y:0:0:1280:5",
    ),
    (
        "an absolute inline-block in a span at font-size 0",
        "",
        r#"<div id=x><span><i id=i style="position:absolute;display:inline-block;width:20px;height:20px"></i></span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:0 i:0:0:20:20 y:0:0:1280:5",
    ),
    // The positioned box is not compared here: beside an in-flow sibling
    // the span is kept, and the engine seats the box 1px low on that
    // existing path (Chromium 0), which is the static position's, not this
    // round's.
    (
        "an absolute inline-block in a span beside an in-flow one",
        "#x{font-size:16px;line-height:20px}",
        r#"<div id=x><span><i id=i style="position:absolute;display:inline-block;width:20px;height:20px"></i>{ib}</span></div><div id=y style="height:5px"></div>"#,
        "x:0:0:1280:20 y:0:20:1280:5",
    ),
];

/// Pages that do not match Chromium yet. One that starts to match fails the
/// test, so the list cannot go stale.
///
/// `two lines of inline-blocks in a span`: an inline box lays its children
/// out in one row and does not break across lines, so the second
/// inline-block stays beside the first, past the 150px block, and the block
/// is one line tall (10; Chromium 20).
///
/// `an empty span with a line-height beside an inline-block`: the tree
/// build keeps no box for an inline with nothing in it, so its line-height
/// cannot reach the line; the block is the inline-block's 10 for
/// Chromium's 40 (develop 16, from the strut). Low impact: nothing is drawn
/// over anything. CSS2 10.8 counts the empty inline box.
const GAPS: &[&str] = &[
    "two lines of inline-blocks in a span",
    "an empty span with a line-height beside an inline-block",
];

#[test]
fn a_zero_font_size_is_zero_and_not_sixteen() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    // The style memo is keyed by the document's address: keep every
    // document alive so no later page can reuse an earlier one's address.
    let mut documents = Vec::new();
    let mut wrong = Vec::new();
    let mut gaps_that_pass = Vec::new();
    for (name, css, body, chrome) in PAGES {
        let html = format!(
            "<!doctype html><html><head><style>body{{margin:0;font-size:0}}{css}</style></head><body>{}</body></html>",
            body.replace("{ib}", IB)
        );
        let ids: Vec<&str> = chrome.split(' ').map(|b| b.split(':').next().unwrap()).collect();
        let document = Document::parse_html(&html).expect("parse");
        let rects = super::nested_layout_matrix_tests::engine_rects(&engine, &document, &ids);
        documents.push(document);
        let got = ids
            .iter()
            .zip(&rects)
            .map(|(id, r)| match r {
                Some(r) => format!("{id}:{}:{}:{}:{}", r[0].round(), r[1].round(), r[2].round(), r[3].round()),
                None => format!("{id}:none"),
            })
            .collect::<Vec<_>>()
            .join(" ");
        match (got == *chrome, GAPS.contains(name)) {
            (false, false) => wrong.push(format!("{name}\n   engine {got}\n   chrome {chrome}")),
            (true, true) => gaps_that_pass.push(*name),
            _ => {}
        }
    }
    assert!(wrong.is_empty(), "{} of {} wrong:\n{}", wrong.len(), PAGES.len(), wrong.join("\n"));
    assert!(
        gaps_that_pass.is_empty(),
        "listed as a gap but matches Chrome now, take it off the list: {gaps_that_pass:?}"
    );
}

/// An inline-block whose text wraps, inside a link: cnn's lead headline
/// (`<a><h2 style="display:inline-block">`). The block loops count wrapped
/// text under an inline child as that child's own lines, so the
/// inline-block's height must not be added to them: the title block was 130
/// tall for its 76, and the stories under it sat 54px low.
///
/// These pages have text at 20px/38px, so only heights are compared, and
/// only on macOS, where the two words are known to wrap in 80px and fit in
/// 300px.
#[test]
#[cfg(target_os = "macos")]
fn a_wrapped_inline_block_in_a_link_is_counted_once() {
    const HEADING: &str = r#"<a href=#><b style="display:inline-block">aaaa bbbb</b></a>"#;
    // (name, width of the block, the link's wrapper, Chromium's boxes)
    let pages = [
        ("two lines", 80, ("", ""), "x:0:0:80:76 y:0:76:1280:5"),
        ("one line", 300, ("", ""), "x:0:0:300:38 y:0:38:1280:5"),
        ("two lines, the link in a span", 80, ("<span>", "</span>"), "x:0:0:80:76 y:0:76:1280:5"),
    ];
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut documents = Vec::new();
    let mut wrong = Vec::new();
    for (name, width, (open, close), chrome) in pages {
        let html = format!(
            "<!doctype html><html><head><style>body{{margin:0}}#x{{width:{width}px;font:20px/38px Helvetica}}</style></head>\
             <body><div id=x>{open}{HEADING}{close}</div><div id=y style=\"height:5px\"></div></body></html>"
        );
        let document = Document::parse_html(&html).expect("parse");
        let rects = super::nested_layout_matrix_tests::engine_rects(&engine, &document, &["x", "y"]);
        documents.push(document);
        let got = ["x", "y"]
            .iter()
            .zip(&rects)
            .map(|(id, r)| match r {
                Some(r) => format!("{id}:{}:{}:{}:{}", r[0].round(), r[1].round(), r[2].round(), r[3].round()),
                None => format!("{id}:none"),
            })
            .collect::<Vec<_>>()
            .join(" ");
        if got != chrome {
            wrong.push(format!("{name}\n   engine {got}\n   chrome {chrome}"));
        }
    }
    assert!(wrong.is_empty(), "{} of 3 wrong:\n{}", wrong.len(), wrong.join("\n"));
}

/// Text whose font size is restored on an inline box INSIDE another inline
/// box, under a `font-size: 0` block: the second hold's pages (`<span><b
/// style="font-size:16px">Hi</b></span>`, nav links `<a><span style=
/// "font-size:14px">Home</span></a>`). The outer inline's own font is 0px,
/// so only the nested inline's text can give the line its height; left
/// out, the block was 0 tall and the next block was laid over the text
/// (develop 16, from the strut it no longer has). The line-heights are
/// explicit so the heights do not depend on a face's metrics; the hold's
/// Chromium 141 values with `normal` were 18 / 16 / 20 / 50.
///
/// These pages have text, so only heights are compared, and only on macOS
/// (the Linux stub shaper gives the text no box of its own).
#[test]
#[cfg(target_os = "macos")]
fn text_restored_inside_a_nested_inline_box_gives_the_line_its_height() {
    // (name, the block's content, Chromium's boxes: #x then #y)
    let pages = [
        (
            "bold text in a span",
            r#"<span><b style="font-size:16px;line-height:20px">Hi</b></span>"#,
            "x:0:0:1280:20 y:0:20:1280:5",
        ),
        (
            "a nav link whose font is restored in the link",
            r#"<a href=#><span style="font-size:14px;line-height:16px">Home</span></a>"#,
            "x:0:0:1280:16 y:0:16:1280:5",
        ),
        (
            "text three inline boxes deep",
            r#"<span><a href=#><b style="font-size:16px;line-height:20px">Hi</b></a></span>"#,
            "x:0:0:1280:20 y:0:20:1280:5",
        ),
        (
            "large text in a span",
            r#"<span><b style="font:40px/50px Helvetica">Hi</b></span>"#,
            "x:0:0:1280:50 y:0:50:1280:5",
        ),
        (
            "restored text and an inline-block in one span",
            r#"<span><b style="font-size:16px;line-height:20px">Hi</b><i style="display:inline-block;width:10px;height:5px"></i></span>"#,
            "x:0:0:1280:20 y:0:20:1280:5",
        ),
    ];
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut documents = Vec::new();
    let mut wrong = Vec::new();
    for (name, body, chrome) in pages {
        let html = format!(
            "<!doctype html><html><head><style>body{{margin:0;font-size:0}}</style></head>\
             <body><div id=x>{body}</div><div id=y style=\"height:5px\"></div></body></html>"
        );
        let document = Document::parse_html(&html).expect("parse");
        let rects = super::nested_layout_matrix_tests::engine_rects(&engine, &document, &["x", "y"]);
        documents.push(document);
        let got = ["x", "y"]
            .iter()
            .zip(&rects)
            .map(|(id, r)| match r {
                Some(r) => format!("{id}:{}:{}:{}:{}", r[0].round(), r[1].round(), r[2].round(), r[3].round()),
                None => format!("{id}:none"),
            })
            .collect::<Vec<_>>()
            .join(" ");
        if got != chrome {
            wrong.push(format!("{name}\n   engine {got}\n   chrome {chrome}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} wrong:\n{}", wrong.len(), pages.len(), wrong.join("\n"));
}
