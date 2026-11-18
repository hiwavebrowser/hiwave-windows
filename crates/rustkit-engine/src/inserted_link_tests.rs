//! Plan row CH2.E20: a `<link rel=stylesheet>` that a script inserts is
//! fetched and its rules apply (HTML, `link` type `stylesheet`: a link that
//! becomes connected obtains its resource; lazy CSS on real sites does this).
//! Until this row the engine looked for stylesheet links once, during the
//! load, so a link a script added was never requested.

use super::*;
use crate::script_net_tests::{serve_routes, Server};

const SHEET: &str = "#t { width: 77px }";

/// One tokio runtime per test: a fetch a live turn starts is bound to the
/// runtime that polled it (`script_net::settle_live`), so every turn of a
/// view, and its load, must come from the same one.
type Runtime = tokio::runtime::Runtime;

fn served_page(page: &str) -> (Engine, EngineViewId, Server, Runtime) {
    let server = serve_routes(vec![
        ("/", "text/html", page.to_string()),
        ("/s.css", "text/css", SHEET.to_string()),
        ("/u.css", "text/css", "#u { width: 55px }".to_string()),
        ("/b.css", "text/css", "#t { width: 55px }".to_string()),
        // `/held...` answers after 300 ms (script_net_tests).
        ("/helds.css", "text/css", SHEET.to_string()),
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

fn read(engine: &mut Engine, view: EngineViewId, script: &str) -> String {
    let got = engine.execute_script(view, &format!("String({script})")).expect("script");
    got.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&got).to_string()
}

fn width(engine: &mut Engine, view: EngineViewId, id: &str) -> String {
    read(engine, view, &format!("document.getElementById('{id}').offsetWidth"))
}

/// Live turns until `#id` is `want` px wide or two seconds pass.
fn pump_until_width(rt: &Runtime, engine: &mut Engine, view: EngineViewId, id: &str, want: &str) -> String {
    let start = std::time::Instant::now();
    loop {
        rt.block_on(engine.pump_live(view, start.elapsed().as_millis() as u64));
        let got = width(engine, view, id);
        if got == want || start.elapsed() > std::time::Duration::from_secs(2) {
            return got;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Live turns for `ms` milliseconds, whatever they bring.
fn pump_for(rt: &Runtime, engine: &mut Engine, view: EngineViewId, ms: u64) {
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(ms) {
        rt.block_on(engine.pump_live(view, start.elapsed().as_millis() as u64));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// `var l`: a stylesheet link to `href`, not yet in the document.
fn link(href: &str) -> String {
    format!("var l = document.createElement('link'); l.setAttribute('rel', 'stylesheet'); l.setAttribute('href', '{href}');")
}

const INSERT: &str = "var l = document.createElement('link'); \
    l.setAttribute('rel', 'stylesheet'); l.setAttribute('href', '/s.css'); \
    document.head.appendChild(l);";

#[test]
fn a_link_a_parser_run_script_appends_is_fetched_and_applies() {
    let page = format!(
        "<!doctype html><html><head></head><body style=margin:0>\
         <div id=t style=height:10px></div><script>{INSERT}</script></body></html>"
    );
    let (mut engine, view, server, rt) = served_page(&page);
    let got = pump_until_width(&rt, &mut engine, view, "t", "77");
    assert_eq!(server.served("/s.css").len(), 1, "the inserted sheet is requested once");
    assert_eq!(got, "77", "its rule applies");
}

#[test]
fn a_link_appended_after_the_load_is_fetched_on_a_live_turn() {
    let (mut engine, view, server, rt) =
        served_page("<!doctype html><html><head></head><body style=margin:0><div id=t style=height:10px></div></body></html>");
    assert_eq!(width(&mut engine, view, "t"), "800");
    engine.execute_script(view, INSERT).expect("insert");
    let got = pump_until_width(&rt, &mut engine, view, "t", "77");
    assert_eq!(server.served("/s.css").len(), 1, "the inserted sheet is requested once");
    assert_eq!(got, "77", "its rule applies");
}

#[test]
fn a_link_already_in_the_markup_is_not_fetched_again_when_another_is_added() {
    let (mut engine, view, server, rt) = served_page(
        "<!doctype html><html><head><link rel=stylesheet href=/s.css></head><body style=margin:0>\
         <div id=t style=height:10px></div><div id=u style=height:10px></div></body></html>",
    );
    assert_eq!(width(&mut engine, view, "t"), "77");
    engine
        .execute_script(
            view,
            "var l = document.createElement('link'); l.rel = 'stylesheet'; \
             l.setAttribute('href', '/u.css'); document.head.appendChild(l);",
        )
        .expect("insert");
    let got = pump_until_width(&rt, &mut engine, view, "u", "55");
    assert_eq!(got, "55", "the added sheet applies");
    assert_eq!(width(&mut engine, view, "t"), "77", "the markup sheet still applies");
    assert_eq!(server.served("/s.css").len(), 1, "the markup sheet is not requested again");
    assert_eq!(server.served("/u.css").len(), 1, "the added sheet is requested once");
}

// ---- Cascade order follows the links' document positions (#984 R1 HOLD).
// Chromium 148.0.7778.216 headless, 800x600; every rule is `#t { width }`
// at one specificity, so the last sheet in document order wins. Before
// this, an inserted link's sheet took a slot after every sheet the load
// found and the same URL in two links was one sheet.

const PAGE_B: &str = "<!doctype html><html><head><link id=b rel=stylesheet href=/b.css></head><body style=margin:0>\
    <div id=t style=height:10px></div></body></html>";
const PAGE_AB: &str = "<!doctype html><html><head><link id=a rel=stylesheet href=/s.css><link id=b rel=stylesheet href=/b.css>\
    </head><body style=margin:0><div id=t style=height:10px></div></body></html>";
const PAGE_A: &str = "<!doctype html><html><head><link id=a rel=stylesheet href=/s.css></head><body style=margin:0>\
    <div id=t style=height:10px></div></body></html>";
const BARE: &str = "<!doctype html><html><head></head><body style=margin:0><div id=t style=height:10px></div></body></html>";

#[test]
fn a_link_inserted_before_a_markup_link_cascades_before_it() {
    let (mut engine, view, server, rt) = served_page(PAGE_B);
    assert_eq!(width(&mut engine, view, "t"), "55");
    let script = link("/s.css") + " document.head.insertBefore(l, document.getElementById('b'));";
    engine.execute_script(view, &script).expect("insert");
    pump_for(&rt, &mut engine, view, 600);
    assert_eq!(server.served("/s.css").len(), 1, "the inserted sheet is fetched");
    assert_eq!(width(&mut engine, view, "t"), "55", "Chromium: the markup link, later in the document, wins");
}

#[test]
fn the_same_url_in_a_second_link_is_a_second_sheet_at_its_own_place() {
    let (mut engine, view, server, rt) = served_page(PAGE_AB);
    assert_eq!(width(&mut engine, view, "t"), "55");
    engine
        .execute_script(view, &(link("/s.css") + " document.head.appendChild(l);"))
        .expect("insert");
    let got = pump_until_width(&rt, &mut engine, view, "t", "77");
    assert_eq!(got, "77", "Chromium: the appended copy of the first sheet, last in the document, wins");
    assert_eq!(server.served("/s.css").len(), 1, "one fetch serves both links (Chromium re-requests; either is fine)");
}

#[test]
fn a_removed_link_takes_its_sheet_with_it() {
    let (mut engine, view, _server, rt) = served_page(BARE);
    engine
        .execute_script(view, &(link("/s.css") + " l.id = 'x'; document.head.appendChild(l);"))
        .expect("insert");
    assert_eq!(pump_until_width(&rt, &mut engine, view, "t", "77"), "77");
    engine.execute_script(view, "document.getElementById('x').remove()").expect("remove");
    assert_eq!(pump_until_width(&rt, &mut engine, view, "t", "800"), "800", "Chromium: the rules go with the link");
}

#[test]
fn a_link_removed_before_its_sheet_lands_never_applies() {
    let (mut engine, view, _server, rt) = served_page(BARE);
    engine
        .execute_script(view, &(link("/helds.css") + " document.head.appendChild(l); l.remove();"))
        .expect("insert and remove");
    pump_for(&rt, &mut engine, view, 800);
    assert_eq!(width(&mut engine, view, "t"), "800");
}

#[test]
fn a_duplicate_link_keeps_its_sheet_when_the_first_link_goes() {
    let (mut engine, view, server, rt) = served_page(PAGE_A);
    assert_eq!(width(&mut engine, view, "t"), "77");
    engine
        .execute_script(
            view,
            &(link("/s.css") + " document.head.appendChild(l); document.getElementById('a').remove();"),
        )
        .expect("insert and remove");
    pump_for(&rt, &mut engine, view, 600);
    assert_eq!(width(&mut engine, view, "t"), "77", "Chromium: the second link's sheet stays");
    assert_eq!(server.served("/s.css").len(), 1);
}

#[test]
fn moving_a_link_moves_its_sheet_in_the_cascade() {
    let (mut engine, view, server, rt) = served_page(PAGE_AB);
    assert_eq!(width(&mut engine, view, "t"), "55");
    engine
        .execute_script(view, "document.head.appendChild(document.getElementById('a'))")
        .expect("move a after b");
    assert_eq!(pump_until_width(&rt, &mut engine, view, "t", "77"), "77", "a moved after b: a wins");
    engine
        .execute_script(view, "document.head.insertBefore(document.getElementById('a'), document.getElementById('b'))")
        .expect("move a before b");
    assert_eq!(pump_until_width(&rt, &mut engine, view, "t", "55"), "55", "a moved before b: b wins");
    assert_eq!(server.served("/s.css").len(), 1, "a move is not a fetch");
    assert_eq!(server.served("/b.css").len(), 1);
}

#[test]
fn a_links_href_change_swaps_its_sheet() {
    let (mut engine, view, server, rt) = served_page(PAGE_A);
    assert_eq!(width(&mut engine, view, "t"), "77");
    engine
        .execute_script(view, "document.getElementById('a').setAttribute('href', '/b.css')")
        .expect("href b");
    assert_eq!(pump_until_width(&rt, &mut engine, view, "t", "55"), "55", "the new href's sheet replaces the old");
    engine
        .execute_script(view, "document.getElementById('a').setAttribute('href', '/s.css')")
        .expect("href a");
    assert_eq!(pump_until_width(&rt, &mut engine, view, "t", "77"), "77", "and back");
    assert_eq!(server.served("/s.css").len(), 1, "a sheet this document fetched is reused");
    assert_eq!(server.served("/b.css").len(), 1);
}

#[test]
fn document_order_wins_over_fetch_order() {
    // A slow link first, then a fast one inserted before it on a later
    // turn: the fast sheet applies alone, then the slow one lands behind
    // it in the cascade and the fast one (last in the document) stays.
    let (mut engine, view, server, rt) = served_page(BARE);
    engine
        .execute_script(view, &(link("/helds.css") + " l.id = 'slow'; document.head.appendChild(l);"))
        .expect("insert slow");
    pump_for(&rt, &mut engine, view, 50);
    engine
        .execute_script(view, &(link("/b.css") + " document.head.insertBefore(l, document.getElementById('slow'));"))
        .expect("insert fast before");
    assert_eq!(pump_until_width(&rt, &mut engine, view, "t", "55"), "55", "the fast sheet applies first");
    pump_for(&rt, &mut engine, view, 700);
    assert_eq!(server.served("/helds.css").len(), 1);
    assert_eq!(width(&mut engine, view, "t"), "77", "Chromium: the slow link is later in the document and wins");
}

#[test]
fn a_link_that_stops_being_a_stylesheet_drops_its_sheet() {
    let (mut engine, view, _server, rt) = served_page(PAGE_A);
    assert_eq!(width(&mut engine, view, "t"), "77");
    engine
        .execute_script(view, "document.getElementById('a').setAttribute('rel', 'preload')")
        .expect("rel preload");
    assert_eq!(pump_until_width(&rt, &mut engine, view, "t", "800"), "800", "Chromium: no stylesheet link, no sheet");
}

#[test]
fn a_disabled_link_is_not_fetched() {
    let (mut engine, view, server, rt) = served_page(BARE);
    engine
        .execute_script(view, &(link("/s.css") + " l.setAttribute('disabled', ''); document.head.appendChild(l);"))
        .expect("insert disabled");
    pump_for(&rt, &mut engine, view, 600);
    assert_eq!(server.served("/s.css").len(), 0, "Chromium does not request a disabled link's sheet");
    assert_eq!(width(&mut engine, view, "t"), "800");
}
