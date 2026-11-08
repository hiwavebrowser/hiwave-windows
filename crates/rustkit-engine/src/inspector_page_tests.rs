//! The Inspector page (`inspector_page.rs`), rendered headless by RustKit
//! for the diagnostics fixture page and checked on its own text runs and
//! boxes.

use super::*;
use rustkit_layout::DisplayCommand;

/// The page `diagnostics_tests.rs` records (its `page()`), as a file.
const TARGET: &str = include_str!("../../../fixtures/inspector/target.html");

fn target_with(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, html).expect("load_html");
    (engine, view)
}

fn dump_of(engine: &Engine, view: EngineViewId, max_elements: usize) -> String {
    let opts = DiagnosticsOptions { max_elements, ..Default::default() };
    serde_json::to_string(&engine.all_element_diagnostics(view, &opts).unwrap()).unwrap()
}

fn target_dump() -> String {
    let (engine, view) = target_with(TARGET);
    dump_of(&engine, view, DEFAULT_MAX_ELEMENTS)
}

fn record<'a>(dump: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    dump["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["identity"]["id"] == id)
        .unwrap_or_else(|| panic!("no record for #{id}"))
}

fn node_of(dump: &str, id: &str) -> usize {
    let v: serde_json::Value = serde_json::from_str(dump).unwrap();
    record(&v, id)["identity"]["node_id"].as_u64().unwrap() as usize
}

/// The Inspector, laid out and painted at 1280x800 with its script run.
fn inspector(page: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 1280, height: 800 })
        .expect("view");
    engine.load_html(view, page).expect("load_html");
    (engine, view)
}

/// `load_html` runs the Inspector's one script, once, with no host call
/// after it (before T8 the host had to call `run_inline_scripts`).
#[test]
fn the_inspector_page_script_runs_exactly_once() {
    let dump = target_dump();
    let (mut engine, view) = inspector(&inspector_page_html(&dump, None, None).unwrap());
    let log: Vec<_> = engine
        .script_log(view)
        .unwrap()
        .iter()
        .map(|r| (r.source.clone(), r.outcome.clone()))
        .collect();
    assert_eq!(log, vec![("inline#1".to_string(), ScriptOutcome::Ran)], "{log:?}");
    let api = engine.execute_script(view, "typeof window.hiwaveInspector.selectNode").unwrap();
    assert!(api.contains("function"), "{api}");
}

#[derive(Debug, Clone)]
struct Run {
    text: String,
    x: f32,
    y: f32,
}

/// Every text run the Inspector painted, in paint order.
fn runs(engine: &Engine, view: EngineViewId) -> Vec<Run> {
    let list = engine.views[&view].display_list.as_ref().expect("a display list");
    list.commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::Text { text, x, y, .. } => Some(Run { text: text.trim().to_string(), x: *x, y: *y }),
            _ => None,
        })
        .collect()
}

fn texts(engine: &Engine, view: EngineViewId) -> Vec<String> {
    runs(engine, view).into_iter().map(|r| r.text).collect()
}

fn has_text(engine: &Engine, view: EngineViewId, want: &str) -> bool {
    texts(engine, view).iter().any(|t| t == want)
}

/// The Inspector's own diagnostics record for one of its elements.
fn own(engine: &Engine, view: EngineViewId, selector: &str) -> serde_json::Value {
    let r = engine
        .element_diagnostics(view, &DiagnosticTarget::Selector(selector.into()), &DiagnosticsOptions::default())
        .expect("diagnostics")
        .unwrap_or_else(|| panic!("the Inspector has no {selector}"));
    serde_json::to_value(r).unwrap()
}

fn border_box(engine: &Engine, view: EngineViewId, selector: &str) -> (f32, f32, f32, f32) {
    let b = &own(engine, view, selector)["box"]["border_box"];
    let f = |k: &str| b[k].as_f64().unwrap() as f32;
    (f("x"), f("y"), f("w"), f("h"))
}

/// The text runs painted inside an element of the Inspector, joined by a
/// space: what the element shows.
fn shown_in(engine: &Engine, view: EngineViewId, selector: &str) -> String {
    let (x, y, w, h) = border_box(engine, view, selector);
    runs(engine, view)
        .into_iter()
        .filter(|r| r.x >= x - 0.5 && r.x < x + w && r.y >= y - 0.5 && r.y < y + h)
        .map(|r| r.text)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A number script computes in the Inspector (`Number(3.0)` and the like).
fn js_number(engine: &mut Engine, view: EngineViewId, script: &str) -> usize {
    let v = engine.execute_script(view, script).unwrap();
    v.trim_start_matches("Number(").trim_end_matches(')').parse::<f64>().unwrap_or_else(|_| panic!("{v}")) as usize
}

/// The tree row (`#tree-rows > div`) that shows node `node_id`.
fn row_of(engine: &mut Engine, view: EngineViewId, node_id: usize) -> Option<String> {
    let n = js_number(
        engine,
        view,
        &format!(
            "(function () {{ var rows = document.getElementById('tree-rows').children;
               for (var i = 0; i < rows.length; i++) if (rows[i].getAttribute('data-node') === '{node_id}') return i + 1;
               return 0; }})()"
        ),
    );
    (n > 0).then(|| format!("#tree-rows > div:nth-child({n})"))
}

fn selected_label(engine: &Engine, view: EngineViewId) -> String {
    shown_in(engine, view, "#selected-label")
}

const KEY_DOWN: u32 = 0x28;
const KEY_UP: u32 = 0x26;
const KEY_LEFT: u32 = 0x25;

fn press(engine: &mut Engine, view: EngineViewId, code: u32, key: &str) {
    engine.handle_text_key(view, code, key, false, false, false);
}

// ---------------------------------------------------------------------------
// The input
// ---------------------------------------------------------------------------

#[test]
fn the_dump_and_diff_schemas_are_checked() {
    let dump = target_dump();
    assert!(inspector_page_html(&dump, None, None).is_ok());
    let wrong = dump.replacen(DIAGNOSTICS_SCHEMA, "hiwave-element-diagnostics/2", 1);
    assert_eq!(
        inspector_page_html(&wrong, None, None),
        Err(InspectorInputError::WrongSchema {
            what: "the diagnostics dump",
            expected: DIAGNOSTICS_SCHEMA,
            found: Some("hiwave-element-diagnostics/2".into()),
        })
    );
    assert!(matches!(inspector_page_html("{", None, None), Err(InspectorInputError::NotJson { .. })));
    let no_list = format!(r#"{{"schema":"{DIAGNOSTICS_SCHEMA}"}}"#);
    assert_eq!(inspector_page_html(&no_list, None, None), Err(InspectorInputError::NoElements));
    let bad_diff = r#"{"schema":"something-else/1"}"#;
    assert!(matches!(
        inspector_page_html(&dump, Some(bad_diff), None),
        Err(InspectorInputError::WrongSchema { what: "the difference result", .. })
    ));
}

#[test]
fn a_wrong_schema_reaching_the_page_is_refused_by_the_page_too() {
    // A host that skips inspector_page_html: the page checks for itself.
    let page = INSPECTOR_PAGE_TEMPLATE.replacen(
        r#"id="hiwave-inspector-input">null<"#,
        r#"id="hiwave-inspector-input">{"schema":"other/1","rows":[]}<"#,
        1,
    );
    let (engine, view) = inspector(&page);
    assert!(has_text(&engine, view, "This dump is not hiwave-element-diagnostics/1; the Inspector cannot read it."));
}

#[test]
fn the_template_alone_says_no_dump_is_loaded_and_asks_for_nothing() {
    let (engine, view) = inspector(INSPECTOR_PAGE_TEMPLATE);
    assert!(has_text(&engine, view, "No diagnostics dump loaded"));
    // No network from the Inspector itself: nothing to fetch, nothing to post.
    for needle in ["src=", "href=", "<link", "fetch(", "XMLHttpRequest", "WebSocket", "import(", "ipc", "innerHTML", "eval(", "Function("] {
        assert!(!INSPECTOR_PAGE_TEMPLATE.contains(needle), "the Inspector page contains {needle:?}");
    }
}

#[test]
fn row_offsets_find_each_record_in_the_dump_text_in_utf16_units() {
    // Non-ASCII (two- and four-byte UTF-8, a surrogate pair in UTF-16) and
    // characters the injector escapes, before the records they offset.
    let html = r#"<!DOCTYPE html><html><body><div id="é" class="😀 a&b"></div><p class="<x>">p</p></body></html>"#;
    let (engine, view) = target_with(html);
    let dump = dump_of(&engine, view, DEFAULT_MAX_ELEMENTS);
    let page = inspector_page_html(&dump, None, None).unwrap();
    let element = |id: &str| {
        let open = format!(r#"<script type="application/json" id="{id}">"#);
        let at = page.find(&open).unwrap() + open.len();
        &page[at..at + page[at..].find("</script>").unwrap()]
    };
    let input: serde_json::Value = serde_json::from_str(element("hiwave-inspector-input")).unwrap();
    let text: Vec<u16> = element("hiwave-inspector-dump").encode_utf16().collect();
    let whole: serde_json::Value = serde_json::from_str(&String::from_utf16(&text).unwrap()).unwrap();
    let original: serde_json::Value = serde_json::from_str(&dump).unwrap();
    assert_eq!(whole, original, "the dump survives its escaping");
    let rows = input["rows"].as_array().unwrap();
    assert_eq!(rows.len(), original["elements"].as_array().unwrap().len());
    for (row, e) in rows.iter().zip(original["elements"].as_array().unwrap()) {
        let (start, end) = (row[0].as_u64().unwrap() as usize, row[1].as_u64().unwrap() as usize);
        let one: serde_json::Value = serde_json::from_str(&String::from_utf16(&text[start..end]).unwrap()).unwrap();
        assert_eq!(&one, e);
        assert_eq!(row[2], e["identity"]["node_id"]);
        assert_eq!(row[5], e["identity"]["tag"]);
    }
    // And the page reads them so.
    let node = node_of(&dump, "é");
    let (engine, view) = inspector(&inspector_page_html(&dump, None, Some(node)).unwrap());
    assert_eq!(selected_label(&engine, view), "div #é .😀.a&b");
}

// ---------------------------------------------------------------------------
// Untrusted text
// ---------------------------------------------------------------------------

#[test]
fn script_tags_in_a_class_name_render_as_text() {
    let html = r#"<!DOCTYPE html><html><body><div id="evil" class="</script><script>window.pwned=1</script>"></div><div id="</script>"></div></body></html>"#;
    let (engine, view) = target_with(html);
    let dump = dump_of(&engine, view, DEFAULT_MAX_ELEMENTS);
    assert!(dump.contains("</script><script>window.pwned=1</script>"), "the record carries the class as written");
    let page = inspector_page_html(&dump, None, Some(node_of(&dump, "evil"))).unwrap();
    // The injected text cannot end its element: the page keeps its own two
    // data elements and one script, and no `pwned` outside an escape.
    assert_eq!(page.matches("</script>").count(), INSPECTOR_PAGE_TEMPLATE.matches("</script>").count());
    assert!(!page.contains("<script>window.pwned"));

    let (mut engine, view) = inspector(&page);
    assert_eq!(engine.execute_script(view, "typeof window.pwned").unwrap(), r#"String("undefined")"#);
    assert_eq!(
        selected_label(&engine, view),
        "div #evil .</script><script>window.pwned=1</script>"
    );
    let all = texts(&engine, view);
    assert!(all.iter().any(|t| t == "#</script>"), "an id with a closing tag is a row's text: {all:?}");
    assert_eq!(js_number(&mut engine, view, "document.getElementsByTagName('script').length"), 3);
}

// ---------------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------------

#[test]
fn the_tree_shows_every_element_indented_by_depth_with_no_box_rows_dimmed() {
    let dump = target_dump();
    let (mut engine, view) = inspector(&inspector_page_html(&dump, None, None).unwrap());
    let parsed: serde_json::Value = serde_json::from_str(&dump).unwrap();
    // head is collapsed: its style child has no row. Everything else does.
    assert!(has_text(&engine, view, "Rows 1–20 of 20"));
    for e in parsed["elements"].as_array().unwrap() {
        let node = e["identity"]["node_id"].as_u64().unwrap() as usize;
        let row = row_of(&mut engine, view, node);
        if e["identity"]["tag"] == "style" {
            assert!(row.is_none(), "head starts collapsed");
            continue;
        }
        let row = row.unwrap_or_else(|| panic!("no row for {}", e["identity"]["selector"]));
        let mut want = vec![e["identity"]["tag"].as_str().unwrap().to_string()];
        if let Some(id) = e["identity"]["id"].as_str() {
            want.push(format!("#{id}"));
        }
        let classes: Vec<&str> = e["identity"]["classes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
        if !classes.is_empty() {
            want.push(format!(".{}", classes.join(".")));
        }
        let shown = shown_in(&engine, view, &row);
        assert!(shown.ends_with(&want.join(" ")), "row {row} shows {shown:?}, want {want:?}");
        // Dimmed exactly when the element has no box.
        let opacity = own(&engine, view, &row)["paint"]["effective_opacity"].as_f64().unwrap();
        let has_box = e["box"]["has_box"].as_bool().unwrap();
        assert_eq!(opacity < 1.0, !has_box, "row for {} (has_box {has_box})", e["identity"]["selector"]);
    }
    // Indentation: 12px per level, from the record's ancestry.
    let flex = row_of(&mut engine, view, node_of(&dump, "flex")).unwrap();
    let f1 = row_of(&mut engine, view, node_of(&dump, "f1")).unwrap();
    let x_of = |engine: &Engine, row: &str, text: &str| {
        let (x, y, w, h) = border_box(engine, view, row);
        runs(engine, view)
            .into_iter()
            .find(|r| r.text == text && r.x >= x && r.x < x + w && r.y >= y && r.y < y + h)
            .unwrap()
            .x
    };
    assert_eq!(x_of(&engine, &f1, "#f1") - x_of(&engine, &flex, "#flex"), 12.0);
}

#[test]
fn arrow_keys_and_clicks_select_and_collapse() {
    let dump = target_dump();
    let (mut engine, view) = inspector(&inspector_page_html(&dump, None, Some(node_of(&dump, "flex"))).unwrap());
    assert_eq!(selected_label(&engine, view), "div #flex");
    press(&mut engine, view, KEY_DOWN, "ArrowDown");
    assert_eq!(selected_label(&engine, view), "div #f1");
    press(&mut engine, view, KEY_UP, "ArrowUp");
    // ArrowLeft collapses #flex; ArrowDown then skips its children.
    press(&mut engine, view, KEY_LEFT, "ArrowLeft");
    assert!(row_of(&mut engine, view, node_of(&dump, "f1")).is_none());
    press(&mut engine, view, KEY_DOWN, "ArrowDown");
    assert_eq!(selected_label(&engine, view), "div #grid");

    // A click on a row selects it.
    let row = row_of(&mut engine, view, node_of(&dump, "fade")).unwrap();
    let (x, y, w, h) = border_box(&engine, view, &row);
    engine.click_at_point(view, x + w / 2.0, y + h / 2.0);
    assert_eq!(selected_label(&engine, view), "div #fade");
    let selected = own(&engine, view, ".row.selected");
    assert_eq!(selected["identity"]["selector"], own(&engine, view, &row)["identity"]["selector"]);
}

#[test]
fn a_truncated_dump_says_so_and_shows_what_it_has() {
    let (engine, view) = target_with(TARGET);
    let dump = dump_of(&engine, view, 5);
    let (mut engine, view) = inspector(&inspector_page_html(&dump, None, None).unwrap());
    let banner = shown_in(&engine, view, "#banner");
    assert!(
        banner.starts_with("Truncated: this dump holds the first 5 of 21 elements"),
        "banner shows {banner:?}"
    );
    assert!(has_text(&engine, view, "Rows 1–4 of 4"), "{:?}", texts(&engine, view));
    // The banner takes a row's height from the tree: 35 rows fit, not 36.
    assert_eq!(js_number(&mut engine, view, "document.getElementById('tree-rows').children.length"), 35);
}

// ---------------------------------------------------------------------------
// The selected element
// ---------------------------------------------------------------------------

fn px(v: &serde_json::Value) -> String {
    v.as_str().unwrap().trim_end_matches("px").to_string()
}

fn num(v: &serde_json::Value) -> String {
    let f = v.as_f64().unwrap();
    if f.fract() == 0.0 { format!("{}", f as i64) } else { format!("{f}") }
}

#[test]
fn the_selected_elements_numbers_are_its_records() {
    let dump = target_dump();
    let parsed: serde_json::Value = serde_json::from_str(&dump).unwrap();
    for id in ["block", "sized", "abs", "f2"] {
        let r = record(&parsed, id);
        let (engine, view) = inspector(&inspector_page_html(&dump, None, Some(node_of(&dump, id))).unwrap());
        let st = &r["style"];
        for (layer, prefix, suffix) in [("margin", "margin-", ""), ("border", "border-", "-width"), ("padding", "padding-", "")] {
            for side in ["top", "right", "bottom", "left"] {
                assert_eq!(
                    shown_in(&engine, view, &format!("#bm-{layer}-{side}")),
                    px(&st[format!("{prefix}{side}{suffix}").as_str()]),
                    "#{id} {layer} {side}"
                );
            }
        }
        let c = &r["box"]["content_box"];
        assert_eq!(shown_in(&engine, view, "#bm-content"), format!("{} × {}", num(&c["w"]), num(&c["h"])), "#{id}");
        let b = &r["box"]["border_box"];
        let scroll = &parsed["page"]["scroll"];
        let page_x = b["x"].as_f64().unwrap() + scroll[0].as_f64().unwrap();
        let page_y = b["y"].as_f64().unwrap() + scroll[1].as_f64().unwrap();
        let shown = shown_in(&engine, view, "#border-box");
        let want = format!(
            "page x {} y {} w {} h {}",
            num(&page_x.into()),
            num(&page_y.into()),
            num(&b["w"]),
            num(&b["h"])
        );
        assert!(shown.starts_with(&format!("Border box {want} ")), "#{id}: {shown:?} should show {want:?}");
        // Every computed value the record carries is listed.
        let computed = shown_in(&engine, view, "#computed");
        for (prop, v) in st.as_object().unwrap() {
            let v = v.as_str().unwrap_or("unknown");
            assert!(computed.contains(&format!("{prop} {v}")), "#{id}: {prop} {v} not in the computed list");
        }
    }
}

#[test]
fn why_fields_are_written_as_sentences() {
    let dump = target_dump();
    let why = |id: &str| {
        let (engine, view) = inspector(&inspector_page_html(&dump, None, Some(node_of(&dump, id))).unwrap());
        // A wider font can soft-wrap the sentence after a hyphen (`nth-` / `child(4)`); the
        // runs are joined with a space, so undo that one join.
        shown_in(&engine, view, "#why").replace("nth- child", "nth-child")
    };
    let gone = why("gone");
    assert!(gone.contains("It has no layout box: display:none, or dropped by the box tree builder."), "{gone}");
    let wide = why("wide");
    assert!(wide.contains("It is clipped by html > body:nth-child(2) > div:nth-child(6)."), "{wide}");
    let far = why("far");
    assert!(far.contains("It is scrolled out of view: its border box lies outside the viewport."), "{far}");
    let pic = why("pic");
    assert!(pic.contains("It is drawn at effective opacity 0.5"), "{pic}");
    assert!(pic.contains("It is painted inside an opacity layer."), "{pic}");
    let abs = why("abs");
    assert!(abs.contains("It is laid out as absolute relative to html > body:nth-child(2) > div:nth-child(4)."), "{abs}");
    assert!(abs.contains("It is positioned (absolute); its z-index is unknown."), "{abs}");
    let block = why("block");
    assert!(block.contains("It contains 4 characters of text (the text itself is not recorded)."), "{block}");
}

// ---------------------------------------------------------------------------
// Selection from the target page
// ---------------------------------------------------------------------------

#[test]
fn a_point_in_the_target_selects_its_element_in_the_inspector() {
    let (target, view) = target_with(TARGET);
    let dump = dump_of(&target, view, DEFAULT_MAX_ELEMENTS);
    let at = |x, y| target.inspector_selection_at(view, x, y).unwrap();
    assert_eq!(at(30.0, 200.0).unwrap().node_id, node_of(&dump, "abs"));
    // Text selects its element; outside the viewport selects nothing.
    let block = at(20.0, 25.0).unwrap();
    assert_eq!(block.node_id, node_of(&dump, "block"));
    assert_eq!(block.selector, "html > body:nth-child(2) > div:nth-child(1)");
    assert_eq!(at(5000.0, 5.0), None);

    // As the initial selection, and in a live Inspector.
    let (mut engine, iview) = inspector(&inspector_page_html(&dump, None, Some(block.node_id)).unwrap());
    assert_eq!(selected_label(&engine, iview), "div #block .a.b");
    let abs = at(30.0, 200.0).unwrap();
    engine.execute_script(iview, &inspector_select_script(abs.node_id)).unwrap();
    assert_eq!(selected_label(&engine, iview), "div #abs");
}

// ---------------------------------------------------------------------------
// The difference from Chromium
// ---------------------------------------------------------------------------

/// An inspector_diff.py result (README "Output") whose first innermost
/// root is #f1's box.
fn f1_diff() -> String {
    serde_json::json!({
        "schema": "hiwave-inspector-diff/1",
        "first": {
            "stage": "GEOMETRY", "selector": "html", "oracle_key": "html", "tag": "html",
            "match_key": "path", "formatting_context": "block", "affected_area": 1200.0,
            "order": 0, "root": true,
            "fields": [{ "field": "box.border_box", "ours": { "x": 0, "y": 0, "w": 800, "h": 2134 },
                         "chrome": { "x": 0, "y": 0, "w": 800, "h": 2136 }, "dx": 0, "dy": 0, "dw": 0, "dh": -2 }]
        },
        "first_innermost": {
            "stage": "GEOMETRY", "selector": "html > body > div:nth-child(2) > div:nth-child(1)",
            "oracle_key": "#f1", "tag": "div", "match_key": "id", "formatting_context": "flex item",
            "affected_area": 600.0, "order": 6, "root": true,
            "fields": [{ "field": "box.border_box", "ours": { "x": 0, "y": 74, "w": 200, "h": 50 },
                         "chrome": { "x": 0, "y": 74, "w": 194, "h": 50 }, "dx": 0, "dy": 0, "dw": 6, "dh": 0 },
                       { "property": "flex-grow", "ours": "1", "chrome": "0" }]
        },
        "roots": [],
        "summary": { "divergent_elements": 3, "roots": 2, "stage_counts": { "DOM": 0, "STYLE": 0, "GEOMETRY": 2 } },
        "matching": {}, "page": {}, "settings": {}
    })
    .to_string()
}

#[test]
fn the_first_difference_is_marked_compared_and_jumped_to() {
    let dump = target_dump();
    let parsed: serde_json::Value = serde_json::from_str(&dump).unwrap();
    let diff: serde_json::Value = serde_json::from_str(&f1_diff()).unwrap();
    let f1 = parsed["elements"].as_array().unwrap().iter().position(|e| e["identity"]["id"] == "f1");
    assert_eq!(diff_root_record(&parsed, &diff), f1);

    // Opened elsewhere, the mark is on #f1's row and on no other.
    let page = inspector_page_html(&dump, Some(&f1_diff()), Some(node_of(&dump, "block"))).unwrap();
    let (mut engine, view) = inspector(&page);
    let row = row_of(&mut engine, view, node_of(&dump, "f1")).unwrap();
    assert_eq!(shown_in(&engine, view, &row), "div #f1 first difference");
    assert_eq!(texts(&engine, view).iter().filter(|t| *t == "first difference").count(), 1);
    assert!(!has_text(&engine, view, "RustKit"), "no comparison while #block is selected");

    // The jump selects it and shows both engines' values side by side.
    let (x, y, w, h) = border_box(&engine, view, "#jump-diff");
    assert!(shown_in(&engine, view, "#jump-diff").contains("Jump to first difference"));
    engine.click_at_point(view, x + w / 2.0, y + h / 2.0);
    assert_eq!(selected_label(&engine, view), "div #f1");
    let compared = shown_in(&engine, view, "#diff-section");
    assert_eq!(
        compared,
        "Difference from Chromium (GEOMETRY) field RustKit Chromium \
         box.border_box 0, 74 200 × 50 0, 74 194 × 50 \
         flex-grow 1 0"
    );
    // 'd' jumps back after moving away.
    press(&mut engine, view, KEY_UP, "ArrowUp");
    assert_ne!(selected_label(&engine, view), "div #f1");
    press(&mut engine, view, 0x44, "d");
    assert_eq!(selected_label(&engine, view), "div #f1");
}

#[test]
fn a_diff_root_missing_from_the_dump_marks_nothing() {
    let dump = target_dump();
    let missing = f1_diff().replace("#f1", "#not-here").replace(
        "html > body > div:nth-child(2) > div:nth-child(1)",
        "html > body > nav",
    );
    let parsed: serde_json::Value = serde_json::from_str(&dump).unwrap();
    // first_innermost is not in the dump; it does not fall back to `first`.
    let diff: serde_json::Value = serde_json::from_str(&missing).unwrap();
    assert_eq!(diff_root_record(&parsed, &diff), None);
    let (engine, view) = inspector(&inspector_page_html(&dump, Some(&missing), None).unwrap());
    assert!(!has_text(&engine, view, "first difference"));
    assert!(!has_text(&engine, view, "Jump to first difference"));
}

// ---------------------------------------------------------------------------
// Console and Network
// ---------------------------------------------------------------------------

#[test]
fn console_and_network_are_placeholders() {
    let (mut engine, view) = inspector(&inspector_page_html(&target_dump(), None, None).unwrap());
    assert!(!has_text(&engine, view, "Console is not available in this build."), "hidden until chosen");
    for (tab, text) in [("#tab-console", "Console is not available in this build."), ("#tab-network", "Network is not available in this build.")] {
        let (x, y, w, h) = border_box(&engine, view, tab);
        engine.click_at_point(view, x + w / 2.0, y + h / 2.0);
        assert!(has_text(&engine, view, text), "{tab}");
        assert!(!has_text(&engine, view, "Computed"), "{tab}: the Elements panel is hidden");
    }
    let (x, y, w, h) = border_box(&engine, view, "#tab-elements");
    engine.click_at_point(view, x + w / 2.0, y + h / 2.0);
    assert!(has_text(&engine, view, "Computed"));
}

// ---------------------------------------------------------------------------
// Size
// ---------------------------------------------------------------------------

#[test]
fn two_thousand_elements_keep_a_fixed_number_of_rows() {
    let mut html = String::from("<!DOCTYPE html><html><head></head><body>");
    for i in 0..400 {
        html.push_str(&format!("<section id=s{i}><p>a</p><div><span>b</span></div><i></i></section>"));
    }
    html.push_str("</body></html>");
    let (target, view) = target_with(&html);
    let dump = dump_of(&target, view, DEFAULT_MAX_ELEMENTS);
    let rows = |engine: &mut Engine, view| js_number(engine, view, "document.getElementById('tree-rows').children.length");
    let (mut engine, iview) = inspector(&inspector_page_html(&dump, None, None).unwrap());
    // 2,003 records (html, head, body, 400 x 5), 36 row elements: (800 - 61) / 20.
    assert!(has_text(&engine, iview, "Rows 1–36 of 2003"), "{:?}", &texts(&engine, iview)[..12]);
    assert_eq!(rows(&mut engine, iview), 36);
    // The window follows the selection; the row elements stay the same.
    for _ in 0..40 {
        press(&mut engine, iview, KEY_DOWN, "ArrowDown");
    }
    // body is visible row 3 (html, head collapsed, body); 40 rows on is row
    // 43, the window's last.
    assert!(has_text(&engine, iview, "Rows 8–43 of 2003"), "{:?}", &texts(&engine, iview)[..12]);
    assert_eq!(rows(&mut engine, iview), 36);
    assert_eq!(selected_label(&engine, iview), "i");
}

// ---------------------------------------------------------------------------
// The parity case
// ---------------------------------------------------------------------------

const CASE_DUMP: &str = include_str!("../../../fixtures/inspector/target.planted.dump.json");
const CASE_DIFF: &str = include_str!("../../../fixtures/inspector/f2_planted.diff.json");
const CASE_PAGE: &str = include_str!("../../../fixtures/inspector/inspector_page.html");

#[test]
fn the_parity_case_is_the_template_with_its_fixture_input() {
    // Chromium's baseline and RustKit's capture must render this exact page.
    assert!(
        inspector_page_html(CASE_DUMP, Some(CASE_DIFF), None).unwrap() == CASE_PAGE,
        "fixtures/inspector/inspector_page.html is stale; regenerate it as fixtures/inspector/README.md says"
    );
}

#[test]
fn a_real_inspector_diff_result_marks_its_root() {
    // tools/inspector/inspector_diff.py's own output (#675) for a planted
    // flex-grow difference on #f2.
    let (mut engine, view) = inspector(CASE_PAGE);
    assert_eq!(selected_label(&engine, view), "div #f2", "the case opens on the difference");
    let row = own(&engine, view, ".row.selected")["identity"]["selector"].as_str().unwrap().to_string();
    assert_eq!(shown_in(&engine, view, &row), "div #f2 first difference");
    assert_eq!(
        shown_in(&engine, view, "#diff-section"),
        "Difference from Chromium (STYLE) field RustKit Chromium flex-grow 2 1"
    );
    press(&mut engine, view, KEY_UP, "ArrowUp");
    assert_eq!(selected_label(&engine, view), "div #f1");
    assert!(!has_text(&engine, view, "RustKit"));
}
