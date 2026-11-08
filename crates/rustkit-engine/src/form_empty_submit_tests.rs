//! #771: a GET form with no successful controls still submits to `action?`.
//!
//! `form_submission` used to return `None` when the entry list was empty, so
//! a search form with only unnamed fields never navigated. The macOS+headless
//! `form_submit_tests` and headless-gated `form_action_url_tests` cover
//! neighbours; this module pins the empty/unnamed path on every `cargo test
//! -p rustkit-engine --lib` (Linux CI included) via a page view — no GPU
//! texture.

use super::*;

fn engine_with(html: &str, url: &str) -> (Engine, EngineViewId, std::rc::Rc<Document>) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_page_view(Bounds::new(0, 0, 800, 600))
        .expect("page view");
    let doc = std::rc::Rc::new(Document::parse_html(html).expect("parse"));
    let view = engine.views.get_mut(&id).expect("view");
    view.document = Some(doc.clone());
    view.url = Some(Url::parse(url).unwrap());
    (engine, id, doc)
}

fn find_by_tag(n: &std::rc::Rc<Node>, tag: &str) -> Option<std::rc::Rc<Node>> {
    if let NodeType::Element { tag_name, .. } = &n.node_type {
        if tag_name.eq_ignore_ascii_case(tag) {
            return Some(n.clone());
        }
    }
    n.children().iter().find_map(|c| find_by_tag(c, tag))
}

#[test]
fn a_get_form_with_only_unnamed_controls_still_submits_to_action() {
    // The #771 regression: successful-controls filtering left the entry list
    // empty, and the early `return None` swallowed the navigation.
    let (mut engine, id, doc) = engine_with(
        r#"<html><body><form action="/search">
             <input value="no-name">
             <input name="off" value="2" disabled>
             <input type="checkbox" name="box" value="3">
           </form></body></html>"#,
        "https://example.com/page",
    );
    let input = find_by_tag(&doc.root(), "input").expect("input");
    engine.views.get_mut(&id).unwrap().lay.focused_node.set(Some(input.id));

    let sub = engine
        .form_submission_for_focus(id)
        .expect("empty successful-controls form must still submit");
    assert_eq!(
        sub.url, "https://example.com/search?",
        "GET with no successful controls goes to action?"
    );
}

#[test]
fn a_get_form_with_a_named_control_keeps_only_successful_entries() {
    // Companion: named/enabled text is kept; unnamed, disabled, unchecked,
    // and non-submitter submit buttons stay out. Same blast radius as #771,
    // previously only under macos+headless `form_submit_tests`.
    let (mut engine, id, doc) = engine_with(
        r#"<html><body><form action="/s">
             <input name="kept" value="1">
             <input value="no-name">
             <input name="off" value="2" disabled>
             <input type="checkbox" name="box" value="3">
             <input type="submit" name="btn" value="Go">
           </form></body></html>"#,
        "https://example.com/",
    );
    let input = find_by_tag(&doc.root(), "input").expect("input");
    engine.views.get_mut(&id).unwrap().lay.focused_node.set(Some(input.id));

    let sub = engine.form_submission_for_focus(id).expect("submission");
    assert!(
        sub.url.contains("kept=1"),
        "named enabled text must submit; got {}",
        sub.url
    );
    for forbidden in ["no-name", "off=", "box=", "btn="] {
        assert!(
            !sub.url.contains(forbidden),
            "{forbidden} must not be submitted; got {}",
            sub.url
        );
    }
}
