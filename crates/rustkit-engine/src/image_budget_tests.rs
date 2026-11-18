//! LAT9: an image's download budget is its own.
//!
//! Seen in a real-window session (2026-11-09) on a search-results page: the
//! page's script swapped each card's thumbnail `src` for a larger image
//! after the load. The pass that fetched the swapped images had one
//! deadline for all of them, set when the pass was planned, and ran its one
//! SVG before any raster. Live turns of 1.5 to 4.7 s left the pass 2 ms of
//! polling per turn; the SVG used the whole budget, and 78 rasters then
//! failed on their first poll, "over the subresource budget", with no
//! request made. Each card had already dropped its thumbnail, so the grid
//! painted blank.
//!
//! Here each image's budget starts when its own request starts, the SVG and
//! raster images share the slots side by side, the fetch and decode run on
//! the network runtime while the page thread is busy, and an `<img>` whose
//! `src` changed keeps painting what it showed until the new image is in.

use super::*;
use crate::script_net_tests::serve_byte_routes;

/// A 1 x 1 PNG.
const PNG: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 252, 207, 192, 80, 15, 0, 4, 133, 1, 128, 132, 169, 140, 33, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;

fn png_route(path: &str) -> (&'static str, &'static str, Vec<u8>) {
    (Box::leak(path.to_string().into_boxed_str()), "image/png", PNG.to_vec())
}

fn runtime() -> &'static tokio::runtime::Runtime {
    thread_local! {
        static RT: &'static tokio::runtime::Runtime = Box::leak(Box::new(
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(),
        ));
    }
    RT.with(|rt| *rt)
}

fn load(
    page: String,
    mut routes: Vec<(&'static str, &'static str, Vec<u8>)>,
    budget_ms: u64,
) -> (Engine, EngineViewId, crate::script_net_tests::Server, std::time::Duration) {
    routes.insert(0, ("/", "text/html", page.into_bytes()));
    let server = serve_byte_routes(routes);
    let config = EngineConfig { subresource_budget_ms: budget_ms, ..EngineConfig::default() };
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let started = std::time::Instant::now();
    runtime().block_on(engine.load_url(view, url)).expect("load_url");
    let took = started.elapsed();
    (engine, view, server, took)
}

fn at(server: &crate::script_net_tests::Server, path: &str) -> Url {
    Url::parse(&format!("http://127.0.0.1:{}{path}", server.port)).unwrap()
}

/// Live turns, `pause` of page-thread work between two, until nothing is
/// in flight or `limit` has passed.
fn turns(engine: &mut Engine, view: EngineViewId, pause: std::time::Duration, limit: std::time::Duration) {
    let rt = runtime();
    let end = std::time::Instant::now() + limit;
    rt.block_on(engine.pump_live(view, 16));
    while std::time::Instant::now() < end {
        std::thread::sleep(pause);
        if rt.block_on(engine.pump_live(view, 16)).in_flight == 0 {
            break;
        }
    }
    rt.block_on(engine.pump_live(view, 16));
}

/// The `<img>` URLs the view's display list paints.
fn painted(engine: &Engine, view: EngineViewId) -> Vec<String> {
    engine.views[&view]
        .display_list
        .as_ref()
        .expect("display list")
        .commands
        .iter()
        .filter_map(|c| match c {
            rustkit_layout::DisplayCommand::Image { url, .. } => Some(url.clone()),
            _ => None,
        })
        .collect()
}

/// The incident's shape: after the load, script adds one SVG that takes
/// longer than the budget and 50 rasters that are quick. The rasters must
/// not wait behind the SVG, nor inherit the clock it spent.
#[test]
fn a_slow_svg_does_not_spend_the_budget_of_fifty_rasters() {
    let script = "var h='<img src=\"/slow.svg?delay=1500\">';\
        for(var i=0;i<50;i++){h+='<img src=\"/r'+i+'.png\">';}\
        document.getElementById('grid').innerHTML=h;";
    let page = "<html><body><div id=grid></div></body></html>".to_string();
    let mut routes: Vec<_> = (0..50).map(|i| png_route(&format!("/r{i}.png"))).collect();
    routes.push(("/slow.svg?delay=1500", "image/svg+xml", SVG.as_bytes().to_vec()));
    let (mut engine, view, server, _) = load(page, routes, 1_000);
    engine.execute_script(view, script).unwrap();
    turns(&mut engine, view, std::time::Duration::from_millis(20), std::time::Duration::from_secs(8));
    let missing: Vec<usize> = (0..50)
        .filter(|i| !engine.image_manager().is_cached(&at(&server, &format!("/r{i}.png"))))
        .collect();
    assert!(missing.is_empty(), "rasters dropped behind the slow SVG: {missing:?}");
    let requested = (0..50).filter(|i| !server.served(&format!("/r{i}.png")).is_empty()).count();
    assert_eq!(requested, 50, "every raster was requested");
}

/// Live turns that hold the page thread longer than the whole budget (the
/// session's were 1.5 to 4.7 s against 8 s): the images are fetched and
/// decoded meanwhile, and the next turn puts them in.
#[test]
fn images_load_while_turns_hold_the_page_longer_than_the_budget() {
    let script = "var h='';\
        for(var i=0;i<20;i++){h+='<img src=\"/t'+i+'.png?delay=150\">';}\
        document.getElementById('grid').innerHTML=h;";
    let page = "<html><body><div id=grid></div></body></html>".to_string();
    let routes: Vec<_> = (0..20).map(|i| png_route(&format!("/t{i}.png?delay=150"))).collect();
    let (mut engine, view, server, _) = load(page, routes, 600);
    engine.execute_script(view, script).unwrap();
    turns(&mut engine, view, std::time::Duration::from_millis(900), std::time::Duration::from_secs(8));
    let missing: Vec<usize> = (0..20)
        .filter(|i| !engine.image_manager().is_cached(&at(&server, &format!("/t{i}.png?delay=150"))))
        .collect();
    assert!(missing.is_empty(), "images lost to turns longer than the budget: {missing:?}");
}

/// Eight images that never answer hold every slot until their own budgets
/// end; the twelve queued behind them then get budgets of their own.
#[test]
fn an_image_timeout_fires_for_that_image_only() {
    let mut body = String::new();
    let mut routes = Vec::new();
    for i in 0..8 {
        body += &format!("<img src=\"/hang{i}.png?delay=3000\">");
        routes.push(png_route(&format!("/hang{i}.png?delay=3000")));
    }
    for i in 0..12 {
        body += &format!("<img src=\"/f{i}.png\">");
        routes.push(png_route(&format!("/f{i}.png")));
    }
    let page = format!("<html><body>{body}</body></html>");
    let (mut engine, view, server, took) = load(page, routes, 500);
    assert!(took < std::time::Duration::from_millis(2_500), "the load waited for the hung images: {took:?}");
    turns(&mut engine, view, std::time::Duration::from_millis(20), std::time::Duration::from_secs(6));
    let missing: Vec<usize> = (0..12)
        .filter(|i| !engine.image_manager().is_cached(&at(&server, &format!("/f{i}.png"))))
        .collect();
    assert!(missing.is_empty(), "quick images failed on the hung ones' clock: {missing:?}");
    let hung = (0..8)
        .filter(|i| engine.image_manager().is_cached(&at(&server, &format!("/hang{i}.png?delay=3000"))))
        .count();
    assert_eq!(hung, 0, "a hung image is dropped at its own budget");
}

/// Chrome keeps an `<img>`'s current image up while the new `src` loads:
/// the box paints the old image until the new one has decoded.
#[test]
fn a_src_swap_keeps_the_old_image_until_the_new_one_decodes() {
    let page = "<html><body><img id=card src=/old.png width=40 height=40></body></html>".to_string();
    let routes = vec![png_route("/old.png"), png_route("/new.png?delay=700")];
    let (mut engine, view, server, _) = load(page, routes, 5_000);
    let old = at(&server, "/old.png").to_string();
    let new = at(&server, "/new.png?delay=700").to_string();
    assert_eq!(painted(&engine, view), [old.clone()], "precondition: the thumbnail painted");
    engine
        .execute_script(view, "document.getElementById('card').src='/new.png?delay=700'")
        .unwrap();
    runtime().block_on(engine.pump_live(view, 16));
    assert_eq!(painted(&engine, view), [old.clone()], "the old image stays up while the new one loads");
    turns(&mut engine, view, std::time::Duration::from_millis(20), std::time::Duration::from_secs(5));
    assert_eq!(painted(&engine, view), [new], "the new image replaced it once decoded");
}

/// What script reads of `#card`, and the `load`/`error` events it heard
/// (the page's `log`).
fn card(engine: &mut Engine, view: EngineViewId) -> String {
    engine
        .execute_script(
            view,
            "(function(){var c=document.getElementById('card');var r=c.getBoundingClientRect();\
             return 'complete='+c.complete+' naturalWidth='+c.naturalWidth+' currentSrc='+c.currentSrc.replace(location.origin,'')\
             +' box='+r.width+'x'+r.height+' events='+window.log.join('|');})()",
        )
        .unwrap()
        .trim_start_matches("String(\"")
        .trim_end_matches("\")")
        .to_string()
}

/// A page with one `<img id=card>` showing `first`, whose `load` and `error`
/// events go to `window.log`.
fn card_page(first: &str, attrs: &str) -> String {
    format!(
        "<html><body><img id=card src={first} {attrs}><script>window.log=[];\
         ['load','error'].forEach(function(t){{document.getElementById('card')\
         .addEventListener(t,function(){{window.log.push(t)}})}});</script></body></html>"
    )
}

const QUICK: std::time::Duration = std::time::Duration::from_millis(20);
const LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// LAT9-r1. Chrome for Testing 148.0.7778.216, measured on a loopback
/// fixture: once the new `src` has settled as failed (here a 404) the old
/// image is gone, the element is the broken image it would have been had
/// the first load failed, `error` fires once, and script reads
/// `complete === true`, `naturalWidth === 0`, `currentSrc` the failed URL.
/// While the request is out, the old image stays and nothing is heard.
#[test]
fn a_src_swap_that_404s_keeps_the_old_image_only_while_it_is_in_flight() {
    let page = card_page("/old.png", "width=40 height=40");
    let (mut engine, view, server, _) = load(page, vec![png_route("/old.png")], 5_000);
    let old = at(&server, "/old.png").to_string();
    let missing = at(&server, "/missing.png?delay=700").to_string();
    engine
        .execute_script(view, "document.getElementById('card').src='/missing.png?delay=700'")
        .unwrap();
    runtime().block_on(engine.pump_live(view, 16));
    std::thread::sleep(std::time::Duration::from_millis(200));
    runtime().block_on(engine.pump_live(view, 16));
    assert!(server.served("/missing.png?delay=700").iter().all(|r| r.answered.is_none()), "precondition: still in flight");
    assert_eq!(painted(&engine, view), [old], "the old image stays up while the new one is in flight");
    let pending = card(&mut engine, view);
    assert!(pending.contains("complete=false") && pending.ends_with("events=load"), "nothing has settled yet: {pending}");

    turns(&mut engine, view, QUICK, LIMIT);
    turns(&mut engine, view, QUICK, std::time::Duration::from_millis(200));
    assert_eq!(painted(&engine, view), [missing], "a settled failure takes the old image down");
    assert_eq!(
        card(&mut engine, view),
        "complete=true naturalWidth=0 currentSrc=/missing.png?delay=700 box=40x40 events=load|error",
    );
}

/// The broken image after a failed swap is the one a failed first load
/// gives: same box, same paint, for an `<img>` with no size of its own.
#[test]
fn a_failed_src_swap_renders_as_a_failed_first_load_does() {
    let (mut first, first_view, first_server, _) = load(card_page("/missing.png", "alt=card"), vec![], 5_000);
    turns(&mut first, first_view, QUICK, std::time::Duration::from_millis(200));
    let first_paint = painted(&first, first_view);
    assert_eq!(first_paint, [at(&first_server, "/missing.png").to_string()]);
    let first_load = card(&mut first, first_view);
    assert!(
        first_load.starts_with("complete=true naturalWidth=0 currentSrc=/missing.png ") && first_load.ends_with("events=error"),
        "a failed first load: {first_load}"
    );

    let (mut engine, view, server, _) = load(card_page("/old.png", "alt=card"), vec![png_route("/old.png")], 5_000);
    engine.execute_script(view, "document.getElementById('card').src='/missing.png'").unwrap();
    turns(&mut engine, view, QUICK, LIMIT);
    turns(&mut engine, view, QUICK, std::time::Duration::from_millis(200));
    assert_eq!(painted(&engine, view), [at(&server, "/missing.png").to_string()]);
    // But for the `load` its first image brought (E54).
    assert_eq!(
        card(&mut engine, view),
        first_load.replace("events=error", "events=load|error"),
        "the swap's broken image is the first load's"
    );
}

/// Bytes that are not an image, a body cut short, and a connection nobody
/// answers settle as a 404 does (Chrome: `error`, broken image, each).
#[test]
fn a_src_swap_that_does_not_decode_or_connect_clears_to_the_broken_image() {
    let mut truncated = PNG.to_vec();
    truncated.truncate(20);
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://127.0.0.1:{}/gone.png", listener.local_addr().unwrap().port())
    };
    for bad in ["/garbage.png", "/truncated.png", dead.as_str()] {
        let routes = vec![
            png_route("/old.png"),
            ("/garbage.png", "image/png", b"this is not an image at all".to_vec()),
            ("/truncated.png", "image/png", truncated.clone()),
        ];
        let (mut engine, view, server, _) = load(card_page("/old.png", "width=40 height=40"), routes, 5_000);
        engine
            .execute_script(view, &format!("document.getElementById('card').src='{bad}'"))
            .unwrap();
        turns(&mut engine, view, QUICK, LIMIT);
        turns(&mut engine, view, QUICK, std::time::Duration::from_millis(200));
        let failed = if bad.starts_with('/') { at(&server, bad).to_string() } else { bad.to_string() };
        assert_eq!(painted(&engine, view), [failed.clone()], "{bad}: the old image is taken down");
        let read = card(&mut engine, view);
        let current = if bad.starts_with('/') { bad.to_string() } else { failed };
        assert_eq!(
            read,
            format!("complete=true naturalWidth=0 currentSrc={current} box=40x40 events=load|error"),
            "{bad}"
        );
    }
}

/// The engine giving up on an image is not the image failing (LAT9-r1,
/// R1 delta): one whose own budget runs out was only slow, and Chrome would
/// still load it. The old image stays up, no `error` reaches the page, and
/// script reads it as still loading (`currentSrc` empty, as Chrome reads a
/// pending swap). The one `load` is the first image's (E54).
#[test]
fn a_src_swap_whose_image_outlives_its_budget_keeps_the_old_image() {
    let page = card_page("/old.png", "width=40 height=40");
    let routes = vec![png_route("/old.png"), png_route("/hang.png?delay=2500")];
    let (mut engine, view, server, _) = load(page, routes, 600);
    let old = at(&server, "/old.png").to_string();
    engine
        .execute_script(view, "document.getElementById('card').src='/hang.png?delay=2500'")
        .unwrap();
    turns(&mut engine, view, QUICK, LIMIT);
    turns(&mut engine, view, QUICK, std::time::Duration::from_millis(200));
    assert!(!server.served("/hang.png?delay=2500").is_empty(), "precondition: it was requested");
    assert!(
        !engine.image_manager().is_cached(&at(&server, "/hang.png?delay=2500")),
        "precondition: the engine gave up on it"
    );
    assert_eq!(painted(&engine, view), [old], "a slow image is not a failed one: the old image stays");
    assert_eq!(
        card(&mut engine, view),
        "complete=false naturalWidth=0 currentSrc= box=40x40 events=load",
    );
}

/// Nor is an image the pass never started. 56 cards swap to images that do
/// not answer: 8 at a time hold the slots for a budget each, so the pass's
/// cap of four budgets passes with some never requested (which ones is the
/// network runtime's order). Every card, given up on either way, keeps its
/// old image, and the page hears nothing.
#[test]
fn a_src_swap_that_misses_the_slot_cap_keeps_the_old_image() {
    const CARDS: usize = 56;
    let cards: String = (0..CARDS).map(|_| "<img class=card src=/old.png width=4 height=4>").collect();
    let page = format!(
        "<html><body>{cards}<script>window.log=[];\
         document.addEventListener('error',function(e){{window.log.push('error')}},true);\
         document.addEventListener('load',function(e){{window.log.push('load')}},true);</script></body></html>"
    );
    let mut routes = vec![png_route("/old.png")];
    routes.extend((0..CARDS).map(|i| png_route(&format!("/hang{i}.png?delay=4000"))));
    let (mut engine, view, server, _) = load(page, routes, 300);
    let old = at(&server, "/old.png").to_string();
    assert_eq!(painted(&engine, view), vec![old.clone(); CARDS], "precondition: every card painted");
    engine
        .execute_script(
            view,
            "var c=document.querySelectorAll('.card');\
             for(var i=0;i<c.length;i++){c[i].src='/hang'+i+'.png?delay=4000';}",
        )
        .unwrap();
    turns(&mut engine, view, QUICK, LIMIT);
    turns(&mut engine, view, QUICK, std::time::Duration::from_millis(200));
    let requested = (0..CARDS).filter(|i| !server.served(&format!("/hang{i}.png?delay=4000")).is_empty()).count();
    assert!(requested >= 8, "precondition: the slots were used ({requested} requested)");
    assert!(requested < CARDS, "precondition: the cap passed with images never started ({requested} requested)");
    assert_eq!(painted(&engine, view), vec![old; CARDS], "an image the engine gave up on is not a failed one");
    let read = engine
        .execute_script(
            view,
            "(function(){var c=document.querySelectorAll('.card'),done=0;\
             for(var i=0;i<c.length;i++){if(c[i].complete)done++;}\
             return 'complete='+done+' events='+window.log.join('|');})()",
        )
        .unwrap();
    assert!(read.contains("complete=0 events=") && !read.contains("error"), "{read}");
}

/// The mark `complete` reads is kept by the element's script wrapper: the
/// same object however the page reaches the element, for the document's
/// life, so a second lookup reads the failure the first heard.
#[test]
fn a_failed_image_reads_the_same_through_every_lookup_of_the_element() {
    let (mut engine, view, _server, _) = load(card_page("/old.png", "width=40 height=40"), vec![png_route("/old.png")], 5_000);
    engine.execute_script(view, "document.getElementById('card').src='/missing.png'").unwrap();
    turns(&mut engine, view, QUICK, LIMIT);
    turns(&mut engine, view, QUICK, std::time::Duration::from_millis(200));
    let read = engine
        .execute_script(
            view,
            "(function(){var a=document.getElementById('card'),b=document.querySelector('img#card');\
             return [a===b,a.complete,b.complete,window.log.join('|')].join(' ');})()",
        )
        .unwrap();
    assert!(read.contains("true true true load|error") && !read.contains("error|error"), "{read}");
    // More turns, more layouts: still one event.
    engine.execute_script(view, "document.body.style.padding='3px'").unwrap();
    turns(&mut engine, view, QUICK, std::time::Duration::from_millis(300));
    assert!(card(&mut engine, view).ends_with("events=load|error"));
}

/// After a failed swap, a `src` that loads shows its image; the failure is
/// not held against the element. Going back to the failed URL fails again.
#[test]
fn a_good_src_after_a_failed_swap_shows_the_new_image() {
    let page = card_page("/old.png", "width=40 height=40");
    let routes = vec![png_route("/old.png"), png_route("/new.png")];
    let (mut engine, view, server, _) = load(page, routes, 5_000);
    engine.execute_script(view, "document.getElementById('card').src='/missing.png'").unwrap();
    turns(&mut engine, view, QUICK, LIMIT);
    turns(&mut engine, view, QUICK, std::time::Duration::from_millis(200));
    assert_eq!(painted(&engine, view), [at(&server, "/missing.png").to_string()]);
    engine.execute_script(view, "document.getElementById('card').src='/new.png'").unwrap();
    turns(&mut engine, view, QUICK, LIMIT);
    turns(&mut engine, view, QUICK, std::time::Duration::from_millis(200));
    assert_eq!(painted(&engine, view), [at(&server, "/new.png").to_string()], "the new image shows");
    let read = card(&mut engine, view);
    assert!(
        read.starts_with("complete=true naturalWidth=1 currentSrc=/new.png ") && read.ends_with("events=load|error|load"),
        "one error, from the failed swap, between the two images' loads: {read}"
    );

    engine.execute_script(view, "document.getElementById('card').src='/missing.png'").unwrap();
    turns(&mut engine, view, QUICK, LIMIT);
    turns(&mut engine, view, QUICK, std::time::Duration::from_millis(200));
    assert_eq!(painted(&engine, view), [at(&server, "/missing.png").to_string()], "the failed URL is broken again");
    let read = card(&mut engine, view);
    assert!(read.starts_with("complete=true naturalWidth=0 ") && read.ends_with("events=load|error|load|error"), "{read}");
}
