//! A form whose `action` is protocol-relative (`//host/path`) is submitted
//! to that host with the document's scheme (WHATWG URL). The Wikipedia
//! portal's search form (`action="//www.wikipedia.org/search-redirect.php"`)
//! went to `https://www.wikipedia.org//www.wikipedia.org/search-redirect.php`.

use super::*;

/// `form.submit()` on a page at `doc_url`, and where the page asked to go.
fn submitted(page: &str, doc_url: &str) -> Option<String> {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    engine.load_html(id, page).expect("load_html");
    let doc = Url::parse(doc_url).unwrap();
    let view = engine.views.get_mut(&id).expect("view");
    view.url = Some(doc.clone());
    let bindings = view.bindings.as_ref().expect("bindings");
    bindings.set_location(&doc).unwrap();
    bindings.evaluate("document.getElementById('f').submit()").unwrap();
    engine.take_script_navigation(id)
}

#[test]
fn a_protocol_relative_action_takes_the_documents_scheme() {
    let page = r#"<html><body><form id="f" action="//example.test/s"></form></body></html>"#;
    assert_eq!(submitted(page, "https://www.example.org/dir/page").as_deref(), Some("https://example.test/s?"));
    assert_eq!(submitted(page, "http://www.example.org/").as_deref(), Some("http://example.test/s?"));
}

#[test]
fn the_wikipedia_portal_search_goes_to_search_redirect() {
    let page = r#"<html><body><form id="f" action="//www.wikipedia.org/search-redirect.php">
<input name="family" value="Wikipedia" type="hidden"><input name="search" value="rust"></form></body></html>"#;
    assert_eq!(
        submitted(page, "https://www.wikipedia.org/").as_deref(),
        Some("https://www.wikipedia.org/search-redirect.php?family=Wikipedia&search=rust")
    );
}
