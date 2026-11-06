//! The developer tools' queries (`devtools.rs`) on a headless page: the
//! element at a point, its geometry, computed style and breadcrumb, the
//! bounded DOM tree, the highlight overlay, the console lines, and the
//! payloads' JSON as the app's Inspector panel reads it.

use super::*;
use crate::inspector::{InspectorRecord, InspectorSnapshot};

/// `#a` fills the top 50px; `#b` is 100x40 with 10px margin, 2px border and
/// 5px padding, so its border box is (10, 60) 114x54.
const PAGE: &str = r#"<!doctype html><html><head><style>
body { margin: 0 }
#a { height: 50px }
#b { margin: 10px; padding: 5px; border: 2px solid black; width: 100px; height: 40px; color: rgb(10, 20, 30) }
</style></head><body><div id="a"></div><div id="b" class="card big"><span id="c">x</span></div><!-- note --></body></html>"#;

fn page(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, html).expect("load_html");
    (engine, view)
}

fn node(engine: &Engine, view: EngineViewId, selector: &str) -> usize {
    engine
        .element_diagnostics(view, &DiagnosticTarget::Selector(selector.into()), &DiagnosticsOptions::default())
        .unwrap()
        .unwrap_or_else(|| panic!("no {selector}"))
        .identity
        .node_id
}

#[test]
fn hit_test_at_a_point_returns_the_element_under_it() {
    let (engine, view) = page(PAGE);
    let (a, b) = (node(&engine, view, "#a"), node(&engine, view, "#b"));
    assert_eq!(engine.devtools_node_at(view, 20.0, 25.0).unwrap(), Some(a));
    // The border, the padding and the content area all belong to #b.
    assert_eq!(engine.devtools_node_at(view, 11.0, 61.0).unwrap(), Some(b));
    assert_eq!(engine.devtools_node_at(view, 60.0, 100.0).unwrap(), Some(b));
    // Its margin does not.
    assert_ne!(engine.devtools_node_at(view, 5.0, 55.0).unwrap(), Some(b));
    // Outside the viewport nothing is hit, as `elementFromPoint`.
    assert_eq!(engine.devtools_node_at(view, -1.0, 10.0).unwrap(), None);
    assert_eq!(engine.devtools_node_at(view, 10.0, 600.0).unwrap(), None);
    // The text `x` selects its span. The point lookup the diagnostics
    // record uses agrees everywhere.
    let c = node(&engine, view, "#c");
    assert_eq!(engine.devtools_node_at(view, 19.0, 70.0).unwrap(), Some(c));
    for (x, y) in [(20.0, 25.0), (11.0, 61.0), (60.0, 100.0), (5.0, 55.0), (19.0, 70.0), (400.0, 400.0)] {
        let slow = engine.inspector_selection_at(view, x, y).unwrap().map(|s| s.node_id);
        assert_eq!(engine.devtools_node_at(view, x, y).unwrap(), slow, "at ({x}, {y})");
    }
}

#[test]
fn element_geometry_and_computed_style_for_the_fixture() {
    let (engine, view) = page(PAGE);
    let b = node(&engine, view, "#b");
    let e = engine.devtools_element(view, b).unwrap().expect("an element");
    assert_eq!((e.tag.as_str(), e.id.as_deref()), ("div", Some("b")));
    assert_eq!(e.classes, vec!["card", "big"]);
    assert_eq!(e.path, format!("rk:{b}"));
    let m = e.box_model.expect("a box");
    assert_eq!((m.margin_top, m.margin_right, m.margin_bottom, m.margin_left), (10.0, 10.0, 10.0, 10.0));
    assert_eq!((m.border_top, m.border_left), (2.0, 2.0));
    assert_eq!((m.padding_top, m.padding_right), (5.0, 5.0));
    assert_eq!((m.width, m.height), (100.0, 40.0));
    assert_eq!(m.border_box, DevtoolsRect { x: 10.0, y: 60.0, width: 114.0, height: 54.0 });
    assert_eq!(m.padding_box, DevtoolsRect { x: 12.0, y: 62.0, width: 110.0, height: 50.0 });
    assert_eq!(m.content_box, DevtoolsRect { x: 17.0, y: 67.0, width: 100.0, height: 40.0 });
    assert_eq!(m.margin_box, DevtoolsRect { x: 0.0, y: 50.0, width: 134.0, height: 74.0 });
    assert_eq!(e.rect, Some(m.border_box));
    assert_eq!(e.computed_styles.get("display").map(String::as_str), Some("block"));
    assert_eq!(e.computed_styles.get("position").map(String::as_str), Some("static"));
    assert!(e.computed_styles.get("color").is_some_and(|c| c.contains("10") && c.contains("30")), "{:?}", e.computed_styles.get("color"));
    for prop in ["width", "height", "margin-top", "padding-left", "font-size", "opacity", "overflow-x"] {
        assert!(e.computed_styles.contains_key(prop), "no computed {prop}");
    }
    // A value the engine cannot give is listed with its reason.
    assert!(e.style_unavailable.get("z-index").is_some_and(|why| why.contains("auto")));
    assert!(e.details_text.contains("/* z-index: unavailable, "), "{}", e.details_text);
    // The cascade cannot say which rule set a property: said, not faked.
    assert!(e.rule_sources.is_empty());
    assert_eq!(e.rule_sources_unavailable, Some(RULE_SOURCES_UNAVAILABLE));
}

#[test]
fn breadcrumb_runs_from_html_to_the_element() {
    let (engine, view) = page(PAGE);
    let c = node(&engine, view, "#c");
    let e = engine.devtools_element(view, c).unwrap().unwrap();
    let labels: Vec<&str> = e.breadcrumb.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["html", "body", "div#b.card.big", "span#c"]);
    assert_eq!(e.breadcrumb.last().unwrap().path, e.path);
    // Each crumb names a node the panel can select in turn.
    let parent = devtools_node_of(&e.breadcrumb[2].path).unwrap();
    assert_eq!(parent, node(&engine, view, "#b"));
    assert!(e.selector.ends_with("> span"), "{}", e.selector);
}

#[test]
fn copy_details_text_has_selector_geometry_and_style() {
    let (engine, view) = page(PAGE);
    let e = engine.devtools_element(view, node(&engine, view, "#b")).unwrap().unwrap();
    let text = &e.details_text;
    assert!(text.starts_with(&format!("selector: {}\n", e.selector)), "{text}");
    assert!(text.contains("path: html > body > div#b.card.big\n"), "{text}");
    assert!(text.contains("border box:  x=10 y=60 w=114 h=54\n"), "{text}");
    assert!(text.contains("margin: 10 10 10 10 | border: 2 2 2 2 | padding: 5 5 5 5\n"), "{text}");
    assert!(text.contains("  display: block;\n"), "{text}");
    // No page text: the span's `x` is not in it.
    assert!(!text.contains("\"x\""));
}

#[test]
fn an_element_with_no_box_says_why() {
    let (engine, view) = page("<html><body><div id=h style='display:none'></div></body></html>");
    let e = engine.devtools_element(view, node(&engine, view, "#h")).unwrap().unwrap();
    assert!(e.box_model.is_none() && e.rect.is_none());
    assert!(e.no_box_reason.is_some());
    assert!(e.details_text.contains("no box: "), "{}", e.details_text);
    assert!(engine.devtools_element(view, usize::MAX).unwrap().is_none());
}

#[test]
fn dom_tree_shows_elements_text_and_comments_and_skips_blank_text() {
    let (engine, view) = page(PAGE);
    let tree = engine.devtools_dom_tree(view, None, None).unwrap().expect("a tree");
    assert_eq!(tree.kind, "document");
    let html = &tree.children[0];
    assert_eq!((html.kind, html.tag.as_str()), ("element", "html"));
    let body = html.children.iter().find(|n| n.tag == "body").expect("body");
    let kinds: Vec<(&str, &str)> = body.children.iter().map(|n| (n.kind, n.tag.as_str())).collect();
    assert_eq!(kinds, [("element", "div"), ("element", "div"), ("comment", "")]);
    assert_eq!(body.children[2].text_content.as_deref(), Some("<text len=6>"));
    let span = &body.children[1].children[0];
    assert_eq!(span.id.as_deref(), Some("c"));
    assert_eq!(span.children[0].text_content.as_deref(), Some("<text len=1>"));
}

#[test]
fn dom_tree_is_depth_bounded_and_reveals_a_picked_node() {
    let mut html = String::from("<html><body>");
    for i in 0..12 {
        html.push_str(&format!("<div id=d{i}>"));
    }
    html.push_str(&"</div>".repeat(12));
    html.push_str("</body></html>");
    let (engine, view) = page(&html);
    fn depth_of(n: &DevtoolsNode) -> usize {
        1 + n.children.iter().map(depth_of).max().unwrap_or(0)
    }
    let tree = engine.devtools_dom_tree(view, None, None).unwrap().unwrap();
    assert_eq!(depth_of(&tree), DEVTOOLS_TREE_DEPTH + 1);
    // Down the last child: html's first is head.
    fn deepest(n: &DevtoolsNode) -> &DevtoolsNode {
        n.children.last().map_or(n, deepest)
    }
    assert_eq!(deepest(&tree).children_omitted, 1, "the cut is counted");
    // Picking the innermost div expands every ancestor down to it.
    let d11 = node(&engine, view, "#d11");
    let revealed = engine.devtools_dom_tree(view, None, Some(d11)).unwrap().unwrap();
    assert_eq!(deepest(&revealed).node_id, d11);
    // Expanding one node asks for its subtree alone.
    let d5 = node(&engine, view, "#d5");
    let sub = engine.devtools_dom_tree(view, Some(d5), None).unwrap().unwrap();
    assert_eq!(sub.id.as_deref(), Some("d5"));
    assert_eq!(engine.devtools_dom_tree(view, Some(usize::MAX), None).unwrap(), None);
}

#[test]
fn dom_tree_stops_at_the_node_cap() {
    let html = format!("<html><body>{}</body></html>", "<p></p>".repeat(DEVTOOLS_TREE_MAX_NODES + 50));
    let (engine, view) = page(&html);
    let tree = engine.devtools_dom_tree(view, None, None).unwrap().unwrap();
    fn count(n: &DevtoolsNode) -> usize {
        1 + n.children.iter().map(count).sum::<usize>()
    }
    assert!(count(&tree) <= DEVTOOLS_TREE_MAX_NODES);
    let body = tree.children[0].children.iter().find(|n| n.tag == "body").unwrap();
    assert_eq!(body.children.len() + body.children_omitted, DEVTOOLS_TREE_MAX_NODES + 50);
    assert!(body.children_omitted > 0);
}

#[test]
fn highlight_adds_an_overlay_to_the_committed_list_only_while_set() {
    let (mut engine, view) = page(PAGE);
    let b = node(&engine, view, "#b");
    let state = &engine.views[&view];
    let base = state.display_list.clone().expect("a display list");
    let layout = state.layout.clone();
    // Not set: the page's own list, not a copy.
    let same = devtools::with_highlight(Some(base.clone()), layout.as_deref(), None).unwrap();
    assert!(Arc::ptr_eq(&same, &base));
    let lit = devtools::with_highlight(Some(base.clone()), layout.as_deref(), Some(b)).unwrap();
    let extra = &lit.commands[base.commands.len()..];
    // Content, padding ring, border ring, margin ring, outline.
    assert_eq!(extra.len(), 1 + 4 + 4 + 4 + 1);
    match extra.last().unwrap() {
        rustkit_layout::DisplayCommand::Border { rect, .. } => {
            assert_eq!((rect.x, rect.y, rect.width, rect.height), (10.0, 60.0, 114.0, 54.0));
        }
        other => panic!("expected the outline, got {other:?}"),
    }
    let generation = engine.views[&view].frame_generation;
    assert!(engine.devtools_highlight(view, Some(b)).unwrap());
    assert!(!engine.devtools_highlight(view, Some(b)).unwrap(), "unchanged");
    assert_eq!(engine.views[&view].frame_generation, generation + 1, "a new frame to commit");
    assert!(engine.devtools_highlight(view, None).unwrap());
}

#[test]
fn a_navigation_clears_the_outline() {
    // #785 R1 HOLD 2: node ids do not survive a navigation, and nothing
    // cleared the outline, so the new page outlined whichever node took the
    // old id and every commit copied the display list.
    let (mut engine, view) = page(PAGE);
    let b = node(&engine, view, "#b");
    assert!(engine.devtools_highlight(view, Some(b)).unwrap());
    engine.load_html(view, PAGE).expect("load_html");
    let state = &engine.views[&view];
    assert_eq!(state.devtools_highlight, None);
    let base = state.display_list.clone().expect("a display list");
    let committed = devtools::with_highlight(Some(base.clone()), state.layout.as_deref(), state.devtools_highlight).unwrap();
    assert!(Arc::ptr_eq(&committed, &base), "the page's own list is committed");
}

#[test]
fn an_idle_console_answer_does_not_wake_the_window() {
    // #785 R1 HOLD 1: every answer woke the window, and the woken pump asked
    // for the console again at once, so an open panel on an idle page kept
    // the window and the page waking each other.
    use crate::page_protocol::devtools_wakes_window as wakes;
    let (mut engine, view) = page("<html><body><p>hi</p></body></html>");
    engine.devtools_attach(view, true).unwrap();
    let since = engine.devtools_console(view, 0).unwrap().next_seq;
    let idle = engine.devtools_console(view, since).unwrap();
    assert!(idle.entries.is_empty() && !idle.page_changed);
    assert!(!wakes(&DevtoolsAnswer::Console(idle), false), "nothing new");
    engine.execute_script(view, "console.log('x')").unwrap();
    let news = engine.devtools_console(view, since).unwrap();
    assert!(wakes(&DevtoolsAnswer::Console(news), false), "a new line");
    assert!(wakes(&DevtoolsAnswer::Done, true), "an outline drawn");
    assert!(!wakes(&DevtoolsAnswer::Done, false), "an acknowledgement");
    assert!(wakes(&DevtoolsAnswer::Node(None), false), "the panel waits on what it asked");
}

#[test]
fn the_panel_shows_text_and_urls_as_shapes() {
    // Pete 2026-10-23: the Inspector is shape-only everywhere. Tree text and
    // comments give their length, not the page's words; a URL loses its
    // query and fragment.
    let (engine, view) = page(PAGE);
    let tree = engine.devtools_dom_tree(view, None, None).unwrap().expect("a tree");
    let body = tree.children[0].children.iter().find(|n| n.tag == "body").expect("body");
    assert_eq!(body.children[2].text_content.as_deref(), Some("<text len=6>"), "the comment");
    assert_eq!(body.children[1].children[0].children[0].text_content.as_deref(), Some("<text len=1>"));
    let at = Some(SourceLocation { url: "https://news.test/app.js?token=s3cret#x".into(), line: 1, column: 2 });
    let snapshot = InspectorSnapshot {
        records: vec![
            record(0, InspectorEvent::NavigationStart { url: Some("https://news.test/a?q=private#top".into()) }),
            record(1, InspectorEvent::Console { level: ConsoleLevel::Log, message: "<string len=5>".into(), truncated: false, location: at, stack: None, error_stack: None }),
        ],
        next_seq: 2,
        dropped: 0,
    };
    let c = devtools_console_entries(&snapshot);
    assert_eq!(c.entries[0].message, "Navigated to https://news.test/a");
    assert_eq!(c.entries[1].stack[0].file, "https://news.test/app.js");
}

#[test]
fn only_a_new_document_resets_the_panel() {
    // #785 R1 NIT: a first layout or the end of the same document's load
    // dropped the selection; they only make the tree stale.
    let at = |event| InspectorSnapshot { records: vec![record(0, event)], next_seq: 1, dropped: 0 };
    let settled = devtools_console_entries(&at(InspectorEvent::FirstLayout));
    assert!(!settled.page_changed && settled.tree_stale);
    let finished = devtools_console_entries(&at(InspectorEvent::LoadFinished { url: None, scripts_over_budget: 0 }));
    assert!(!finished.page_changed && finished.tree_stale);
    let committed = devtools_console_entries(&at(InspectorEvent::NavigationCommit { url: None, status: Some(200) }));
    assert!(committed.page_changed && committed.tree_stale);
}

fn record(seq: u64, event: InspectorEvent) -> InspectorRecord {
    InspectorRecord { seq, doc: 0, t_ms: 0.0, event }
}

#[test]
fn console_lines_from_inspector_records() {
    let at = Some(SourceLocation { url: "https://news.test/app.js".into(), line: 3, column: 7 });
    let snapshot = InspectorSnapshot {
        records: vec![
            record(0, InspectorEvent::NavigationStart { url: Some("https://news.test/".into()) }),
            record(1, InspectorEvent::Console { level: ConsoleLevel::Warn, message: "<string len=5>".into(), truncated: false, location: at.clone(), stack: None, error_stack: None }),
            record(2, InspectorEvent::FirstLayout),
            record(3, InspectorEvent::ScriptError {
                name: "TypeError".into(),
                message: None,
                location: at.clone(),
                stack: None,
                unhandled_rejection: true,
                script: "inline#1".into(),
                value: "error".into(),
                message_len: Some(12),
                inserted_by: None,
            }),
            record(4, InspectorEvent::UnsupportedApi { interface: "Window".into(), member: "fetchLater".into(), location: None }),
            record(5, InspectorEvent::LoadFinished { url: None, scripts_over_budget: 2 }),
            record(6, InspectorEvent::LoadFinished { url: None, scripts_over_budget: 0 }),
        ],
        next_seq: 7,
        dropped: 1,
    };
    let c = devtools_console_entries(&snapshot);
    let lines: Vec<(u64, &str, &str)> = c.entries.iter().map(|e| (e.id, e.level, e.kind)).collect();
    assert_eq!(
        lines,
        [
            (0, "info", "navigation_start"),
            (1, "warn", "console"),
            (3, "error", "script_error"),
            (4, "warn", "unsupported_api"),
            (5, "warn", "load_finished"),
        ]
    );
    assert_eq!(c.entries[2].message, "Uncaught (in promise) TypeError (message hidden, 12 chars) [inline#1]");
    assert_eq!(c.entries[2].stack, [DevtoolsFrame { file: "https://news.test/app.js".into(), line: 3, column: 7 }]);
    assert_eq!(c.entries[3].message, "Not supported by RustKit: Window.fetchLater");
    assert!(c.entries[4].message.starts_with("2 script(s) skipped or stopped"));
    assert_eq!((c.next_seq, c.dropped, c.page_changed, c.tree_stale), (7, 1, false, true));
    let quiet = devtools_console_entries(&InspectorSnapshot { records: vec![], next_seq: 7, dropped: 0 });
    assert!(quiet.entries.is_empty() && !quiet.page_changed && !quiet.tree_stale);
}

#[test]
fn console_reads_since_and_with_the_ring_on_closing_forgets_nothing() {
    // T6b, `HIWAVE_DEVTOOLS_RING=1`: the tab ring records before the panel
    // opens, and closing the panel keeps what was recorded (it used to
    // empty the buffer).
    let inspector = crate::inspector::InspectorConfig { buffer: true, ..crate::inspector::InspectorConfig::tab() };
    let mut engine = Engine::new(EngineConfig { inspector, ..EngineConfig::default() }).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, "<html><body><p>hi</p></body></html>").expect("load_html");
    let console_levels = |c: &crate::DevtoolsConsole| -> Vec<&'static str> {
        c.entries.iter().filter(|e| e.kind == "console").map(|e| e.level).collect()
    };
    engine.execute_script(view, "console.log('before')").unwrap();
    assert_eq!(console_levels(&engine.devtools_console(view, 0).unwrap()), ["log"], "the tab ring records while closed");
    engine.devtools_attach(view, true).unwrap();
    engine.execute_script(view, "console.log('one'); console.error('two')").unwrap();
    let c = engine.devtools_console(view, 0).unwrap();
    assert_eq!(console_levels(&c), ["log", "log", "error"]);
    engine.execute_script(view, "console.info('three')").unwrap();
    let next = engine.devtools_console(view, c.next_seq).unwrap();
    assert_eq!(console_levels(&next), ["info"]);
    engine.devtools_attach(view, false).unwrap();
    engine.execute_script(view, "console.log('after')").unwrap();
    assert_eq!(
        console_levels(&engine.devtools_console(view, 0).unwrap()),
        ["log", "log", "error", "info", "log"],
        "closing forgets nothing"
    );
}

#[test]
fn payloads_serialize_in_the_panels_field_names() {
    let (engine, view) = page(PAGE);
    let b = node(&engine, view, "#b");
    let e = serde_json::to_value(engine.devtools_element(view, b).unwrap().unwrap()).unwrap();
    assert_eq!(e["path"], format!("rk:{b}"));
    assert_eq!(e["nodeId"], b);
    assert_eq!(e["boxModel"]["marginTop"], 10.0);
    assert_eq!(e["boxModel"]["paddingLeft"], 5.0);
    assert_eq!(e["boxModel"]["borderBox"]["width"], 114.0);
    assert_eq!(e["rect"]["x"], 10.0);
    assert_eq!(e["computedStyles"]["display"], "block");
    assert_eq!(e["breadcrumb"][2]["label"], "div#b.card.big");
    assert!(e["detailsText"].as_str().unwrap().starts_with("selector: "));
    assert_eq!(e["ruleSourcesUnavailable"], RULE_SOURCES_UNAVAILABLE);

    let t = serde_json::to_value(engine.devtools_dom_tree(view, None, None).unwrap().unwrap()).unwrap();
    let html = &t["children"][0];
    assert_eq!((html["type"].as_str(), html["tag"].as_str()), (Some("element"), Some("html")));
    assert!(html["childrenOmitted"].is_u64() && html["path"].as_str().unwrap().starts_with("rk:"));

    let c = devtools_console_entries(&InspectorSnapshot {
        records: vec![record(9, InspectorEvent::Console { level: ConsoleLevel::Log, message: "m".into(), truncated: true, location: None, stack: None, error_stack: None })],
        next_seq: 10,
        dropped: 0,
    });
    let c = serde_json::to_value(c).unwrap();
    assert_eq!(c["nextSeq"], 10);
    assert_eq!(c["pageChanged"], false);
    assert_eq!(c["entries"][0], serde_json::json!({"id": 9, "doc": 0, "level": "log", "kind": "console", "message": "m …", "stack": []}));
}

#[test]
fn panel_paths_round_trip() {
    assert_eq!(devtools_node_of(&devtools_path(42)), Some(42));
    for bad in ["42", "rk:", "rk:x", "html > body", "rk:-1"] {
        assert_eq!(devtools_node_of(bad), None, "{bad}");
    }
}
