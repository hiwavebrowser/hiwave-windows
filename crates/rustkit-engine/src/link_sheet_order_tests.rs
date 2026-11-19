//! #984 second HOLD (CH2.E20): `<link rel=stylesheet>` sheets cascade at
//! their document position among the `<style>` sheets, inserted links
//! included, and a link's `media` attribute decides whether its sheet
//! applies, re-evaluated on resize (HTML 4.2.4 `media`; CSSOM: the sheet's
//! media list). Until this, every external sheet came after every `<style>`
//! sheet and a `media=print` link styled the screen.
//!
//! Every expectation is Chrome for Testing 148.0.7778.216 headless at
//! 800x600 (`scratchpad/fix39/e20_media_probe.mjs`): `/a.css` makes `#t`
//! 77px wide, `/b.css` 55px, the `<style>` sheets 10px or 55px.

use super::*;
use crate::script_net_tests::{serve_routes, Server};

type Runtime = tokio::runtime::Runtime;

const A: &str = "<link rel=stylesheet href=/a.css>";

fn served(head: &str, body: &str) -> (Engine, EngineViewId, Server, Runtime) {
    let page = format!(
        "<!doctype html><html><head><style>body{{margin:0}}</style>{head}</head><body>\
         <div id=t style=height:10px></div>{body}</body></html>"
    );
    let server = serve_routes(vec![
        ("/", "text/html", page),
        ("/a.css", "text/css", "#t { width: 77px }".to_string()),
        ("/b.css", "text/css", "#t { width: 55px }".to_string()),
    ]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server, rt)
}

fn width(engine: &mut Engine, view: EngineViewId) -> String {
    let got = engine
        .execute_script(view, "String(document.getElementById('t').offsetWidth)")
        .expect("script");
    got.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&got).to_string()
}

/// Live turns until `#t` is `want` px wide or two seconds pass.
fn pump_until(rt: &Runtime, engine: &mut Engine, view: EngineViewId, want: &str) -> String {
    let start = std::time::Instant::now();
    loop {
        rt.block_on(engine.pump_live(view, start.elapsed().as_millis() as u64));
        let got = width(engine, view);
        if got == want || start.elapsed() > std::time::Duration::from_secs(2) {
            return got;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn pump_for(rt: &Runtime, engine: &mut Engine, view: EngineViewId, ms: u64) {
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(ms) {
        rt.block_on(engine.pump_live(view, start.elapsed().as_millis() as u64));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn style_and_link_sheets_cascade_in_document_order() {
    let cases = [
        ("style 10, link A, style 55", format!("<style>#t{{width:10px}}</style>{A}<style>#t{{width:55px}}</style>"), "", "55"),
        ("link A, style 55", format!("{A}<style>#t{{width:55px}}</style>"), "", "55"),
        ("style 55, link A", format!("<style>#t{{width:55px}}</style>{A}"), "", "77"),
        ("link A in head, style 55 in body after #t", A.to_string(), "<style>#t{width:55px}</style>", "55"),
    ];
    let mut wrong = Vec::new();
    for (name, head, body, want) in cases {
        let (mut engine, view, _server, _rt) = served(&head, body);
        let got = width(&mut engine, view);
        if got != want {
            wrong.push(format!("{name}: got {got}, Chromium {want}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn an_inserted_link_or_style_takes_its_document_position() {
    let (mut engine, view, _server, rt) = served("<style id=s>#t{width:55px}</style>", "");
    engine
        .execute_script(
            view,
            "var l=document.createElement('link'); l.rel='stylesheet'; l.href='/a.css'; \
             document.head.insertBefore(l, document.getElementById('s'))",
        )
        .expect("insert before");
    pump_for(&rt, &mut engine, view, 600);
    assert_eq!(width(&mut engine, view), "55", "a link inserted before a <style> cascades before it");

    let (mut engine, view, _server, rt) = served("<style id=s>#t{width:55px}</style>", "");
    engine
        .execute_script(
            view,
            "var l=document.createElement('link'); l.rel='stylesheet'; l.href='/a.css'; document.head.appendChild(l)",
        )
        .expect("append");
    assert_eq!(pump_until(&rt, &mut engine, view, "77"), "77", "a link appended after a <style> cascades after it");

    let (mut engine, view, _server, _rt) = served(A, "");
    assert_eq!(width(&mut engine, view), "77");
    engine
        .execute_script(
            view,
            "var s=document.createElement('style'); s.textContent='#t{width:55px}'; document.head.appendChild(s)",
        )
        .expect("append style");
    assert_eq!(width(&mut engine, view), "55", "a <style> appended after a link cascades after it");
}

#[test]
fn a_links_media_attribute_decides_whether_its_sheet_applies() {
    let link = |media: &str| format!("<link rel=stylesheet href=/a.css media=\"{media}\">");
    let cases = [
        ("print", "800"),
        ("screen", "77"),
        ("all", "77"),
        ("(min-width: 900px)", "800"),
        ("(max-width: 900px)", "77"),
        ("foo bar", "800"),
        ("PRINT, screen", "77"),
    ];
    let mut wrong = Vec::new();
    for (media, want) in cases {
        let (mut engine, view, server, _rt) = served(&link(media), "");
        let got = width(&mut engine, view);
        if got != want {
            wrong.push(format!("media={media:?}: got {got}, Chromium {want}"));
        }
        if server.served("/a.css").len() != 1 {
            wrong.push(format!("media={media:?}: fetched {} times, Chromium 1", server.served("/a.css").len()));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_links_media_follows_script_and_the_viewport() {
    let (mut engine, view, _server, _rt) = served("<link id=l rel=stylesheet href=/a.css media=print>", "");
    assert_eq!(width(&mut engine, view), "800");
    engine
        .execute_script(view, "document.getElementById('l').removeAttribute('media')")
        .expect("remove media");
    assert_eq!(width(&mut engine, view), "77", "media removed by script");

    let (mut engine, view, _server, _rt) = served("<link id=l rel=stylesheet href=/a.css>", "");
    assert_eq!(width(&mut engine, view), "77");
    engine
        .execute_script(view, "document.getElementById('l').setAttribute('media', 'print')")
        .expect("set media");
    assert_eq!(width(&mut engine, view), "800", "media=print set by script");

    let (mut engine, view, _server, rt) = served("", "");
    engine
        .execute_script(
            view,
            "var l=document.createElement('link'); l.rel='stylesheet'; l.href='/a.css'; l.setAttribute('media','print'); \
             document.head.appendChild(l)",
        )
        .expect("insert print link");
    pump_for(&rt, &mut engine, view, 600);
    assert_eq!(width(&mut engine, view), "800", "an inserted media=print link does not style the screen");

    let (mut engine, view, _server, _rt) = served("<link rel=stylesheet href=/a.css media=\"(min-width: 900px)\">", "");
    assert_eq!(width(&mut engine, view), "800");
    engine
        .resize_view(view, Bounds { x: 0, y: 0, width: 1000, height: 600 })
        .expect("resize");
    assert_eq!(width(&mut engine, view), "77", "the media query is re-evaluated on resize");
}
