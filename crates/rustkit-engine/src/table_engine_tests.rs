//! The engine half of table layout (#626): UA defaults for the table
//! elements, `border-spacing` / `border-collapse`, the HTML presentational
//! attributes, spans, and box-tree generation for the table display types.
//!
//! The tree is built from parsed HTML with `build_layout_from_document` and
//! laid out with rustkit-layout's collapse entry point (the one pages take).
//! Cells hold fixed-size blocks, not text, wherever a rectangle is asserted,
//! so the numbers do not depend on the host's fonts. These are unit tests of
//! the wiring, not oracle comparisons with Chromium.

use super::*;

fn build(html: &str) -> LayoutBox {
    let document = Rc::new(Document::parse_html(html).expect("html"));
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut root = engine.build_layout_from_document(&document, &[]);
    root.set_viewport(800.0, 600.0);
    let cb = Dimensions {
        content: rustkit_layout::Rect::new(0.0, 0.0, 800.0, 0.0),
        ..Default::default()
    };
    root.layout_with_collapse(
        &cb,
        &mut rustkit_layout::MarginCollapseContext::new(),
        &mut rustkit_layout::FloatContext::new(),
    );
    root
}

fn all<'a>(b: &'a LayoutBox, pred: &dyn Fn(&LayoutBox) -> bool, out: &mut Vec<&'a LayoutBox>) {
    if pred(b) {
        out.push(b);
    }
    for c in &b.children {
        all(c, pred, out);
    }
}

fn with_display(root: &LayoutBox, d: rustkit_css::Display) -> Vec<&LayoutBox> {
    let mut out = Vec::new();
    all(
        root,
        &|b| b.style.display == d && matches!(b.box_type, BoxType::Block),
        &mut out,
    );
    out
}

fn cells(root: &LayoutBox) -> Vec<&LayoutBox> {
    with_display(root, rustkit_css::Display::TableCell)
}

fn tables(root: &LayoutBox) -> Vec<&LayoutBox> {
    with_display(root, rustkit_css::Display::Table)
}

/// The text boxes under `b`, trimmed, in tree order.
fn text_of(b: &LayoutBox) -> String {
    let mut out = Vec::new();
    all(b, &|b| matches!(b.box_type, BoxType::Text(_)), &mut out);
    out.iter()
        .filter_map(|b| match &b.box_type {
            BoxType::Text(t) => Some(t.trim().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[track_caller]
fn close(got: f32, want: f32, what: &str) {
    assert!((got - want).abs() < 0.01, "{what}: got {got}, want {want}");
}

const BOX: &str = "<style>i { display: block; width: 50px; height: 20px }</style>";

#[test]
fn the_issue_repro_puts_cells_side_by_side() {
    let root = build("<table><tr><td>C</td><td>D</td></tr><tr><td>E</td><td>F</td></tr></table>");
    let cs = cells(&root);
    assert_eq!(cs.len(), 4, "four cells");
    let names: Vec<String> = cs.iter().map(|c| text_of(c)).collect();
    assert_eq!(names, ["C", "D", "E", "F"]);
    let r: Vec<_> = cs.iter().map(|c| c.dimensions.border_box()).collect();
    close(r[1].y, r[0].y, "C and D share a row");
    assert!(
        r[1].x > r[0].right() - 0.01,
        "D is right of C: {:?} {:?}",
        r[0],
        r[1]
    );
    close(r[2].x, r[0].x, "E under C");
    close(r[3].x, r[1].x, "F under D");
    assert!(r[2].y >= r[0].bottom(), "the second row is below the first");
    assert!(
        r[0].width < 400.0,
        "cells are not full width: {}",
        r[0].width
    );
}

#[test]
fn ua_defaults_give_tables_their_displays_and_spacing() {
    let root = build(&format!(
        "{BOX}<table><caption>cap</caption><colgroup><col></colgroup>\
         <thead><tr><th><i></i></th></tr></thead>\
         <tbody><tr><td><i></i></td></tr></tbody>\
         <tfoot><tr><td><i></i></td></tr></tfoot></table>"
    ));
    use rustkit_css::Display;
    let t = tables(&root)[0];
    assert_eq!(t.style.border_spacing, (2.0, 2.0));
    assert_eq!(
        t.style.border_collapse,
        rustkit_css::BorderCollapse::Separate
    );
    assert_eq!(t.style.box_sizing, rustkit_css::BoxSizing::BorderBox);
    let kinds: Vec<Display> = t.children.iter().map(|c| c.style.display).collect();
    assert_eq!(
        kinds,
        [
            Display::TableCaption,
            Display::TableColumnGroup,
            Display::TableHeaderGroup,
            Display::TableRowGroup,
            Display::TableFooterGroup,
        ]
    );
    assert_eq!(
        t.children[1].children[0].style.display,
        Display::TableColumn
    );
    let cs = cells(&root);
    let th = cs[0];
    assert_eq!(th.style.font_weight, rustkit_css::FontWeight::BOLD);
    assert_eq!(th.style.text_align, rustkit_css::TextAlign::Center);
    for c in &cs {
        assert_eq!(c.style.padding_left, rustkit_css::Length::Px(1.0));
        // `vertical-align: inherit` from the row group's `middle`.
        assert_eq!(c.style.vertical_align, rustkit_css::VerticalAlign::Middle);
    }
    // 2px spacing, 1px cell padding, 50px content: the cell is at 2,
    // 52 wide, and the table is 2 + 52 + 2.
    let tb = t.dimensions.border_box();
    close(tb.width, 56.0, "table width");
    close(cs[1].dimensions.border_box().x - tb.x, 2.0, "cell x");
    close(cs[1].dimensions.border_box().width, 52.0, "cell width");
}

#[test]
fn an_author_valign_on_a_row_reaches_its_cells() {
    let root = build("<style>tr { vertical-align: top }</style><table><tr><td>x</td></tr></table>");
    assert_eq!(
        cells(&root)[0].style.vertical_align,
        rustkit_css::VerticalAlign::Top
    );
}

#[test]
fn border_spacing_and_collapse_parse_and_inherit() {
    let root = build(
        "<style>table { border-spacing: 10px 4px } .c { border-collapse: collapse; border-spacing: 1em }</style>\
         <table><tr><td><table class=c><tr><td>x</td></tr></table></td></tr></table>",
    );
    let ts = tables(&root);
    assert_eq!(ts[0].style.border_spacing, (10.0, 4.0));
    assert_eq!(
        ts[1].style.border_collapse,
        rustkit_css::BorderCollapse::Collapse
    );
    assert_eq!(ts[1].style.border_spacing, (16.0, 16.0));
    // Inherited by everything inside.
    assert_eq!(cells(&root)[0].style.border_spacing, (10.0, 4.0));
}

#[test]
fn colspan_rowspan_and_span_reach_the_boxes() {
    let root = build(
        "<table><colgroup span=2></colgroup><col span=0>\
         <tr><td colspan=2 rowspan=3>a</td><td colspan=0 rowspan=0>b</td></tr></table>",
    );
    let cs = cells(&root);
    assert_eq!((cs[0].table_span.colspan, cs[0].table_span.rowspan), (2, 3));
    assert_eq!((cs[1].table_span.colspan, cs[1].table_span.rowspan), (1, 0));
    let t = tables(&root)[0];
    assert_eq!(t.children[0].table_span.colspan, 2, "colgroup span");
    assert_eq!(t.children[1].table_span.colspan, 1, "col span=0 is 1");
}

#[test]
fn table_presentational_attributes_map_to_style() {
    let root = build(&format!(
        "{BOX}<table width=300 height=80 border=2 cellpadding=5 cellspacing=0 align=right>\
         <tr><td width=120 height=40 align=center valign=bottom bgcolor=ff0000><i></i></td>\
         <td bgcolor=\"#00ff00\"><i></i></td></tr></table>"
    ));
    let t = tables(&root)[0];
    assert_eq!(t.style.width, rustkit_css::Length::Px(300.0));
    assert_eq!(t.style.height, rustkit_css::Length::Px(80.0));
    assert_eq!(t.style.border_left_width, rustkit_css::Length::Px(2.0));
    assert_eq!(t.style.border_spacing, (0.0, 0.0));
    assert_eq!(t.float, rustkit_css::Float::Right);
    let cs = cells(&root);
    let c = &cs[0].style;
    assert_eq!(c.padding_top, rustkit_css::Length::Px(5.0));
    assert_eq!(c.border_top_width, rustkit_css::Length::Px(1.0));
    assert_eq!(c.width, rustkit_css::Length::Px(120.0));
    assert_eq!(c.height, rustkit_css::Length::Px(40.0));
    assert_eq!(c.text_align, rustkit_css::TextAlign::Center);
    assert_eq!(c.vertical_align, rustkit_css::VerticalAlign::Bottom);
    assert_eq!(c.background_color, rustkit_css::Color::from_rgb(255, 0, 0));
    assert_eq!(
        cs[1].style.background_color,
        rustkit_css::Color::from_rgb(0, 255, 0)
    );
    // Laid out: 300 wide (border box), floated right in 800 - 2*8 body.
    let tb = t.dimensions.border_box();
    close(tb.width, 300.0, "table width");
    close(tb.right(), 792.0, "floated right");
    close(tb.height, 80.0, "table height");
    // The first cell is 120 + 2*5 padding + 2*1 border; the row is
    // 80 - 2*2 border tall.
    close(cs[0].dimensions.border_box().width, 132.0, "cell width");
    close(cs[0].dimensions.border_box().height, 76.0, "cell height");
}

#[test]
fn author_css_beats_presentational_attributes() {
    let root = build(
        "<style>table { width: 200px } td { padding: 0; background-color: blue }</style>\
         <table width=300 cellpadding=9><tr><td bgcolor=red>x</td></tr></table>",
    );
    assert_eq!(tables(&root)[0].style.width, rustkit_css::Length::Px(200.0));
    let c = &cells(&root)[0].style;
    assert_eq!(c.padding_top, rustkit_css::Length::Zero);
    assert_eq!(c.background_color, rustkit_css::Color::from_rgb(0, 0, 255));
}

#[test]
fn cells_with_different_attributes_do_not_share_a_style() {
    let root =
        build("<table><tr><td bgcolor=red>a</td><td bgcolor=lime>b</td><td>c</td></tr></table>");
    let cs = cells(&root);
    assert_ne!(cs[0].style.background_color, cs[1].style.background_color);
    assert_eq!(
        cs[2].style.background_color,
        rustkit_css::Color::TRANSPARENT
    );
}

#[test]
fn a_nested_tables_cellpadding_is_its_own() {
    let root = build(
        "<table cellpadding=7><tr><td><table><tr><td>x</td></tr></table></td><td>y</td></tr></table>",
    );
    let cs = cells(&root);
    // Tree order: outer cell 1, inner cell, outer cell 2.
    assert_eq!(cs[0].style.padding_top, rustkit_css::Length::Px(7.0));
    assert_eq!(cs[1].style.padding_top, rustkit_css::Length::Px(1.0));
    assert_eq!(cs[2].style.padding_top, rustkit_css::Length::Px(7.0));
}

#[test]
fn align_center_centres_the_table() {
    let root = build(&format!(
        "{BOX}<table align=center cellspacing=0><tr><td style=\"padding:0\"><i></i></td></tr></table>"
    ));
    let tb = tables(&root)[0].dimensions.border_box();
    close(tb.width, 50.0, "table width");
    close(tb.x, 8.0 + (784.0 - 50.0) / 2.0, "centred in the body");
}

#[test]
fn empty_cells_and_cols_keep_their_place() {
    let root = build(&format!(
        "{BOX}<table cellspacing=0><col width=100><tr><td style=\"padding:0\"></td><td style=\"padding:0\"><i></i></td></tr></table>"
    ));
    let cs = cells(&root);
    assert_eq!(cs.len(), 2, "the empty cell is a box");
    close(cs[0].dimensions.border_box().width, 100.0, "col width");
    close(
        cs[1].dimensions.border_box().x - cs[0].dimensions.border_box().x,
        100.0,
        "second column",
    );
}

#[test]
fn css_table_displays_on_divs_get_anonymous_rows_and_tables() {
    let root = build(&format!(
        "{BOX}<div id=w><div style=\"display:table-cell\"><i></i></div>\n  \
         <div style=\"display:table-cell\"><i></i></div></div>"
    ));
    let ts = tables(&root);
    assert_eq!(ts.len(), 1, "one anonymous table");
    assert_eq!(ts[0].node_id, None, "anonymous");
    let cs = cells(&root);
    close(
        cs[1].dimensions.border_box().x,
        cs[0].dimensions.border_box().right(),
        "side by side",
    );
}

#[test]
fn a_clearfix_table_pseudo_is_empty() {
    let root = build(
        "<style>.cf::after { content: \" \"; display: table; clear: both } \
         .f { float: left; width: 10px; height: 30px }</style>\
         <div class=cf><div class=f></div></div><p id=after>x</p>",
    );
    let t = tables(&root)[0];
    close(
        t.dimensions.border_box().height,
        0.0,
        "the pseudo table is empty",
    );
    close(t.dimensions.border_box().width, 0.0, "and has no width");
}

#[test]
fn html_attribute_parsers() {
    assert_eq!(parse_html_non_negative_integer(" 3px"), Some(3));
    assert_eq!(parse_html_non_negative_integer("+2"), Some(2));
    assert_eq!(parse_html_non_negative_integer("-1"), None);
    assert_eq!(parse_html_non_negative_integer(""), None);
    assert_eq!(html_dimension("50%", true).as_deref(), Some("50%"));
    assert_eq!(html_dimension(" 300", true).as_deref(), Some("300px"));
    assert_eq!(html_dimension("12.5px", true).as_deref(), Some("12.5px"));
    assert_eq!(html_dimension("0", true), None);
    assert_eq!(html_dimension("0", false).as_deref(), Some("0px"));
    assert_eq!(html_dimension("auto", false), None);
    assert_eq!(html_legacy_color("ffcc00"), "#ffcc00");
    assert_eq!(html_legacy_color("red"), "red");
    assert_eq!(html_legacy_color("#abc"), "#abc");
    let mut attrs = HashMap::new();
    attrs.insert("border".to_string(), String::new());
    assert_eq!(table_border_attribute(&attrs), Some(1), "<table border>");
    attrs.insert("border".to_string(), "0".to_string());
    assert_eq!(table_border_attribute(&attrs), Some(0));
    assert!(
        table_presentational_hints("table", &attrs).is_empty(),
        "border=0 draws nothing"
    );
}
