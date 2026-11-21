//! M2: a navigation releases the document it replaces.
//!
//! The E35 tab-cycle run saw the app's heap grow with every navigation of
//! the one content view (~10 MB a navigation on a one-paragraph page, ~20 MB
//! on a page of 2000 rows) and never come back. A malloc-stack diff of 30
//! navigations put almost all of it in the script realms each new document
//! gets (the bindings' install scripts, compiled again per document): the
//! realm of every replaced document stayed alive, and the `Document` its
//! DOM host holds with it.
//!
//! JavaScript objects are freed by a collection, not when the last Rust
//! owner lets go, so each test forces one before it looks.

use super::*;
use std::rc::Weak;

fn engine_and_view() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    (engine, view)
}

fn current_document(engine: &Engine, view: EngineViewId) -> Weak<Document> {
    Rc::downgrade(engine.views.get(&view).and_then(|v| v.document.as_ref()).expect("a document"))
}

const NEXT: &str = "<html><body><p>two</p></body></html>";

fn replaced_document_is_released(first_page: &str) {
    let (mut engine, view) = engine_and_view();
    engine.load_html(view, first_page).expect("first load");
    let first = current_document(&engine, view);
    for _ in 0..3 {
        engine.load_html(view, NEXT).expect("next load");
    }
    boa_gc::force_collect();
    assert_eq!(first.strong_count(), 0, "the replaced document is still held after a collection");
    // The view's own document is untouched.
    assert!(current_document(&engine, view).upgrade().is_some());
}

#[test]
fn navigation_releases_a_plain_document() {
    replaced_document_is_released("<html><body><p>one</p></body></html>");
}

#[test]
fn navigation_releases_a_scripted_document() {
    // A listener, a pending timer, an error with a stack and a console call:
    // the page's script holds on to its realm in every way it commonly does.
    replaced_document_is_released(
        "<html><body><p id=p>one</p><script>\
         var p = document.getElementById('p');\
         p.addEventListener('click', function () { p.textContent = 'clicked'; });\
         setTimeout(function () { p.textContent = 'later'; }, 60000);\
         try { null.x; } catch (e) { console.log(e.stack); }\
         Promise.resolve().then(function () { p.title = 'then'; });\
         </script></body></html>",
    );
}

#[test]
fn navigation_releases_every_replaced_document() {
    let (mut engine, view) = engine_and_view();
    let mut replaced = Vec::new();
    for i in 0..10 {
        engine.load_html(view, &format!("<html><body><p>page {i}</p><script>window.n = {i};</script></body></html>")).expect("load");
        replaced.push(current_document(&engine, view));
    }
    engine.load_html(view, NEXT).expect("last load");
    boa_gc::force_collect();
    let live = replaced.iter().filter(|d| d.strong_count() > 0).count();
    assert_eq!(live, 0, "{live} of 10 replaced documents are still held");
}
