//! CH3.A6b: resource and navigation URLs on info/warn lines are scheme,
//! host and path. Signed CDN URLs carry tokens in the query; a marker in
//! a stylesheet, image, SVG, web-font or page URL's query or fragment must
//! never reach the default (info) log. The whole URL stays on debug.

use super::error_page_script_tests::{engine, serve};
use super::*;

const MARK: &str = "a6b-secret";

#[derive(Clone, Default)]
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Everything logged at info and above while `run` ran on this thread.
fn info_log(run: impl FnOnce()) -> String {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    let bytes = captured.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

fn site(path: &str, _cookie: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
    match path {
        "/" => (
            200,
            "text/html",
            vec![],
            format!(
                r#"<html><head>
<link rel="stylesheet" href="/a.css?tok={MARK}#{MARK}">
<link rel="stylesheet" href="/gone.css?tok={MARK}">
<style>@font-face {{ font-family: Mk; src: url(/f.woff2?tok={MARK}); }} p {{ font-family: Mk; }}</style>
</head><body><p>text</p>
<img src="/i.png?tok={MARK}" width="10" height="10">
<img src="/s.svg?tok={MARK}" width="10" height="10">
<img src="/t.png?tok={MARK}" width="10" height="10">
</body></html>"#
            ),
        ),
        "/a.css" => (200, "text/css", vec![], "body { color: red; }".into()),
        "/i.png" => (200, "image/png", vec![], "not a png".into()),
        "/s.svg" => (200, "image/svg+xml", vec![], "<svg".into()),
        "/t.png" => (
            200,
            "image/svg+xml",
            vec![],
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#.into(),
        ),
        "/f.woff2" => (200, "font/woff2", vec![], "not a font".into()),
        "/nothing" => (204, "text/plain", vec![], String::new()),
        "/err" => (404, "text/html", vec![], "<html><body><p>missing</p></body></html>".into()),
        _ => (404, "text/plain", vec![], "not found".into()),
    }
}

fn leaks(log: &str) -> Vec<&str> {
    log.lines().filter(|line| line.contains(MARK)).collect()
}

#[test]
fn a_marker_in_a_resource_url_query_never_reaches_the_info_log() {
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    let log = info_log(|| {
        rt.block_on(engine.load_url(view, server.url(&format!("/?page={MARK}"))))
            .expect("page");
    });
    // The lines this guards ran (else the test proves nothing).
    for needle in ["Loading external stylesheet", "Failed to fetch stylesheet", "Loading image", "Loading SVG image", "Loading web font"] {
        assert!(log.contains(needle), "no `{needle}` line was logged:\n{log}");
    }
    assert!(log.contains("/a.css"), "the info line keeps the path:\n{log}");
    assert_eq!(leaks(&log), Vec::<&str>::new(), "query/fragment marker on an info line");
}

#[test]
fn a_marker_in_a_no_content_or_error_page_url_never_reaches_the_info_log() {
    let server = serve(site);
    let (mut engine, view, rt) = engine();
    let log = info_log(|| {
        rt.block_on(engine.load_url(view, server.url("/")))
            .expect("page");
        let _ = rt.block_on(engine.load_url(view, server.url(&format!("/nothing?tok={MARK}#{MARK}"))));
        let _ = rt.block_on(engine.load_url(view, server.url(&format!("/err?tok={MARK}"))));
    });
    for needle in ["Navigation answered with no content", "Showing the body of an error response"] {
        assert!(log.contains(needle), "no `{needle}` line was logged:\n{log}");
    }
    assert_eq!(leaks(&log), Vec::<&str>::new(), "query/fragment marker on an info/warn line");
}
