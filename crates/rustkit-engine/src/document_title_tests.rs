//! `document.title` is the document's `<title>` element (HTML §3.1.3).
//!
//! It was a plain property of the page's `document`: reading it never saw
//! a `<title>` script changed, and setting it changed no element, so a
//! page that sets its title with none in the markup had no `<title>` at
//! all (a Next.js page does on every navigation: Speedometer's
//! NewsSite-Next was one element short of Chrome's document).
//!
//! Expected lines: docs/diagnostics/2026-11-12/document_title_cft148.txt,
//! `window.report()` of the page beside it in Chrome for Testing
//! 148.0.7778.216. One line there is Chrome's own, not the standard's:
//! set with no `<title>` and no `<head>`, the value is still read back.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/document_title.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/document_title_cft148.txt");

#[test]
fn document_title_reads_and_writes_the_title_element() {
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let id = engine.create_page_view(Bounds::new(0, 0, 800, 600)).expect("page view");
    engine.load_html(id, PAGE).expect("load_html");
    let out = engine.execute_script(id, "String(window.report())").expect("script");
    let got = out
        .strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .map(|s| s.replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\"))
        .unwrap_or(out);
    let got: Vec<&str> = got.lines().collect();
    let want: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).collect();
    assert_eq!(got.len(), want.len(), "lines: {got:#?}");
    let wrong: Vec<String> = got.iter().zip(&want).filter(|(g, w)| g != w).map(|(g, w)| format!("got  {g}\n     want {w}")).collect();
    assert!(wrong.is_empty(), "the lines that differ from Chrome 148:\n{}", wrong.join("\n"));
}

/// A page whose markup has a `<title>` reads it, as before.
#[test]
fn a_title_in_the_markup_is_read() {
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let id = engine.create_page_view(Bounds::new(0, 0, 800, 600)).expect("page view");
    engine.load_html(id, "<html><head><title> From  markup </title></head><body></body></html>").expect("load_html");
    assert_eq!(engine.execute_script(id, "document.title").expect("script"), "String(\"From markup\")");
    assert_eq!(
        engine.execute_script(id, "document.title = 'changed'; document.querySelector('title').textContent + '|' + document.querySelectorAll('title').length").expect("script"),
        "String(\"changed|1\")"
    );
}

/// Following the title costs a page nothing unless a write touched a
/// `<title>`: the document is read for its title only after a script turn
/// whose writes insert, remove or change one. Before, every DOM-writing
/// turn of a page with no `<title>` walked the whole document.
#[test]
fn only_a_write_that_touches_a_title_reads_the_documents_title() {
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let id = engine.create_page_view(Bounds::new(0, 0, 800, 600)).expect("page view");
    let html = format!("<html><head></head><body>{}<div id=\"t\">0</div></body></html>", "<p>x</p>".repeat(5000));
    engine.load_html(id, &html).expect("load_html");
    let title = |engine: &Engine| engine.views.get(&id).and_then(|v| v.title.clone());
    assert_eq!(title(&engine), None);

    let before = title_reads();
    for i in 0..10 {
        let write = format!("document.getElementById('t').textContent = '{i}'; document.body.appendChild(document.createElement('i')); document.body.setAttribute('data-n', '{i}'); 0");
        engine.execute_script(id, &write).expect("script");
    }
    assert_eq!(title_reads() - before, 0, "a page with no title was read for one");

    // A title appears, changes, and goes: each is followed.
    let mut step = |engine: &mut Engine, script: &str| {
        engine.execute_script(id, script).expect("script");
        title(engine)
    };
    assert_eq!(step(&mut engine, "document.title = 'now'; 0"), Some("now".to_string()));
    assert_eq!(step(&mut engine, "document.querySelector('title').firstChild.data = 'its text'; 0"), Some("its text".to_string()));
    assert_eq!(step(&mut engine, "document.querySelector('title').appendChild(document.createTextNode(' grew')); 0"), Some("its text grew".to_string()));
    assert_eq!(step(&mut engine, "document.head.innerHTML = ''; 0"), None, "removed with its parent's children");
    assert_eq!(
        step(&mut engine, "window.d = document.createElement('div'); d.innerHTML = '<span><title>deep</title></span>'; document.body.appendChild(d); 0"),
        Some("deep".to_string()),
        "inserted inside a subtree"
    );
    assert_eq!(step(&mut engine, "d.remove(); 0"), None, "removed inside a subtree");
    // With none again, other writes cost nothing again.
    let before = title_reads();
    engine.execute_script(id, "document.getElementById('t').textContent = 'again'; 0").expect("script");
    assert_eq!(title_reads() - before, 0);
    assert_eq!(step(&mut engine, "document.head.innerHTML = '<meta charset=utf-8><title>parsed</title>'; 0"), Some("parsed".to_string()));
}
