//! Z-plan C3: what `getComputedStyle` reads back for `display`, `z-index`
//! and custom properties. The computed `display` was published from the
//! engine's layout enum, so `<li>` read `block` and `flow-root` read
//! `block`; an element the box tree dropped (an empty `<div>`) read `none`;
//! `z-index: 0` read `auto`; `getPropertyValue('--x')` read `''`. Expected
//! values are Chromium 143's answers on this hand-made page unless a test
//! says it pins a gap.

use super::*;

const PAGE: &str = r#"<!doctype html><style>
    body { --x: 7px; --y: a b; --fgColor-default: red; margin: 0 }
    #own { --x: 9px; --z: 1 }
    #ib { display: inline-block } #if { display: inline-flex } #ig { display: inline-grid }
    #fr { display: flow-root } #ct { display: contents } #wb { display: -webkit-box }
    #ili { display: inline list-item } #bf { display: block flow } #ifr { display: inline flow-root }
    #ali { display: list-item }
    #z0 { position: relative; z-index: 0 } #zauto { position: relative } #zneg { position: absolute; z-index: -1 }
    #zstatic { z-index: 0 }
    #hidden { display: none; color: rgb(1, 2, 3) }
    </style><body>
    <div id="div">div</div><div id="empty"></div><span id="span">span</span><span id="emptyspan"></span>
    <ul><li id="li">li</li></ul><div id="ali">ali</div>
    <table id="table"><caption id="caption">c</caption><thead id="thead"><tr id="tr"><th id="th">h</th></tr></thead>
    <tbody id="tbody"><tr><td id="td">d</td></tr></tbody><tfoot id="tfoot"><tr><td>f</td></tr></tfoot></table>
    <div id="ib">ib</div><div id="if">if</div><div id="ig">ig</div><div id="fr">fr</div>
    <div id="ct"><span>ct</span></div><div id="wb">wb</div><span id="ili">ili</span>
    <div id="bf">bf</div><span id="ifr">ifr</span>
    <div id="z0">z0</div><div id="zauto">zauto</div><div id="zneg">zneg</div><div id="zstatic">zs</div>
    <div id="hidden">h<span id="inhidden">i</span></div>
    <div id="inherits">v</div><div id="own">o<span id="ownchild">oc</span></div>
    </body>"#;

fn loaded() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 1280, 800))
        .expect("headless view");
    engine.load_html(id, PAGE).expect("load_html");
    (engine, id)
}

fn eval(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let out = engine
        .execute_script(id, &format!("String(eval({script:?}))"))
        .expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .unwrap_or_else(|| panic!("{script} evaluated to {out}"))
        .replace("\\\"", "\"")
}

fn read(engine: &mut Engine, id: EngineViewId, element: &str, property: &str) -> String {
    eval(
        engine,
        id,
        &format!(
            "getComputedStyle(document.getElementById('{element}')).getPropertyValue('{property}')"
        ),
    )
}

/// Every `(element, property, Chrome's value)` in `cases` reads the same
/// here; the failures are reported together, one per line.
fn assert_reads(engine: &mut Engine, id: EngineViewId, cases: &[(&str, &str, &str)]) {
    let mut wrong = Vec::new();
    for (element, property, chrome) in cases {
        let got = read(engine, id, element, property);
        if got != *chrome {
            wrong.push(format!(
                "#{element} {property}: chrome {chrome:?}, engine {got:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} reads differ from Chrome:\n{}",
        wrong.len(),
        cases.len(),
        wrong.join("\n")
    );
}

#[test]
fn builtin_elements_read_chromes_display_keyword() {
    let (mut e, id) = loaded();
    assert_reads(
        &mut e,
        id,
        &[
            ("div", "display", "block"),
            ("span", "display", "inline"),
            ("li", "display", "list-item"),
            ("table", "display", "table"),
            ("caption", "display", "table-caption"),
            ("thead", "display", "table-header-group"),
            ("tbody", "display", "table-row-group"),
            ("tfoot", "display", "table-footer-group"),
            ("tr", "display", "table-row"),
            ("td", "display", "table-cell"),
            ("th", "display", "table-cell"),
        ],
    );
}

#[test]
fn authored_display_values_read_back_as_chrome_serialises_them() {
    let (mut e, id) = loaded();
    assert_reads(
        &mut e,
        id,
        &[
            ("ib", "display", "inline-block"),
            ("if", "display", "inline-flex"),
            ("ig", "display", "inline-grid"),
            ("fr", "display", "flow-root"),
            ("ct", "display", "contents"),
            ("wb", "display", "-webkit-box"),
            ("ali", "display", "list-item"),
            ("ili", "display", "inline list-item"),
            ("bf", "display", "block"),
            ("ifr", "display", "inline-block"),
        ],
    );
}

/// GCS1 lane, 2026-10-30: an empty `<div>` read `display: none`. The box
/// tree drops an empty block with nothing visible about it, and an element
/// with no box after a layout was reported as not rendered.
#[test]
fn an_element_the_box_tree_dropped_reads_its_computed_style() {
    let (mut e, id) = loaded();
    assert_reads(
        &mut e,
        id,
        &[
            ("empty", "display", "block"),
            ("empty", "visibility", "visible"),
            ("emptyspan", "display", "inline"),
            ("emptyspan", "color", "rgb(0, 0, 0)"),
        ],
    );
}

/// LIMIT: the box tree drops an empty block with nothing visible about it,
/// so it has no used width; Chrome lays it out and reads the containing
/// block's. Keeping the box is a layout change (margins, line boxes), not
/// a publish fix; this row reports `0px`.
#[test]
#[ignore = "an empty block the box tree dropped has no used width (layout follow-up)"]
fn a_dropped_empty_block_reads_its_used_width() {
    let (mut e, id) = loaded();
    assert_reads(&mut e, id, &[("empty", "width", "1280px")]);
}

#[test]
fn a_display_none_element_reads_none_with_its_cascaded_style() {
    let (mut e, id) = loaded();
    assert_reads(
        &mut e,
        id,
        &[
            ("hidden", "display", "none"),
            ("hidden", "color", "rgb(1, 2, 3)"),
        ],
    );
}

/// LIMIT (LIMITS_LEDGER, #501): a descendant of a `display: none` subtree
/// reads its own computed value in Chrome, not `none`. The build stops at
/// the `display: none` element and styles nothing under it, so a descendant
/// has no published style and reads the "no box" fallback. Styling hidden
/// subtrees is cascade work on content that never paints; not this row.
#[test]
#[ignore = "nothing under a display: none element is styled (cascade follow-up)"]
fn a_descendant_of_a_display_none_subtree_reads_its_own_display() {
    let (mut e, id) = loaded();
    assert_reads(
        &mut e,
        id,
        &[
            ("inhidden", "display", "inline"),
            ("inhidden", "color", "rgb(1, 2, 3)"),
        ],
    );
}

/// LIMITS_LEDGER (#501): `z-index: 0` read `auto`.
#[test]
fn z_index_zero_reads_zero_and_auto_reads_auto() {
    let (mut e, id) = loaded();
    assert_reads(
        &mut e,
        id,
        &[
            ("z0", "z-index", "0"),
            ("zauto", "z-index", "auto"),
            ("zneg", "z-index", "-1"),
            ("zstatic", "z-index", "0"),
            ("div", "z-index", "auto"),
        ],
    );
}

/// LIMITS_LEDGER (#501): custom properties were not published, so
/// `getPropertyValue('--x')` read `''` where Chrome reads the cascaded
/// (inherited) value.
#[test]
fn custom_properties_read_the_cascaded_value() {
    let (mut e, id) = loaded();
    assert_reads(
        &mut e,
        id,
        &[
            ("inherits", "--x", "7px"),
            // Custom property names are case-sensitive (Primer's
            // `--fgColor-*`): no kebab-casing on the way in.
            ("inherits", "--fgColor-default", "red"),
            ("ownchild", "--fgColor-default", "red"),
            ("inherits", "--fgcolor-default", ""),
            ("inherits", "--y", "a b"),
            ("own", "--x", "9px"),
            ("own", "--z", "1"),
            ("ownchild", "--x", "9px"),
            ("ownchild", "--y", "a b"),
            ("ownchild", "--z", "1"),
            ("inherits", "--z", ""),
            ("inherits", "--missing", ""),
        ],
    );
}
