//! A news feed that fills after load, scrolled before and after it fills,
//! must paint what it filled inside the viewport the user scrolled to.
//!
//! Seen in the real window on yahoo.com (2026-10): two wheel flicks down
//! right after the page went idle left 99.3% of the viewport white, the
//! footer and one ad label visible, the feed rows between them blank. With
//! the window held by page script the scroll landed late and the feed
//! showed. Each test here copies one common feed pattern, scrolls the way
//! a flick does (many small wheel events between live turns), lets the page
//! settle, and draws the window's frame at the scrolled offset.

use super::*;
use crate::script_net_tests::serve_routes;

const W: u32 = 400;
const H: u32 = 300;

/// A 400x300 view on `page`, served with `extra` routes beside it.
fn load(page: String, extra: Vec<(&'static str, &'static str, String)>) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let mut routes = vec![("/", "text/html", page)];
    routes.extend(extra);
    let server = serve_routes(routes);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: W, height: H })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    runtime().block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

/// One runtime per test thread: image fetches a live turn starts are
/// polled by the turns after it.
fn runtime() -> &'static tokio::runtime::Runtime {
    thread_local! {
        static RT: &'static tokio::runtime::Runtime = Box::leak(Box::new(
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(),
        ));
    }
    RT.with(|rt| *rt)
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

/// Live turns for `ms` of page time, 16 ms each, as the window's loop runs
/// them while nothing else happens.
fn settle(engine: &mut Engine, view: EngineViewId, ms: u64) {
    let rt = runtime();
    let mut left = ms;
    while left > 0 {
        let step = left.min(16);
        rt.block_on(engine.pump_live(view, step));
        left -= step;
    }
    // Answers still in flight get their turns.
    for _ in 0..50 {
        if rt.block_on(engine.pump_live(view, 16)).in_flight == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    rt.block_on(engine.pump_live(view, 16));
}

/// Two flicks down: `events` wheel events of `step` px each, a live turn
/// between each two, as a trackpad's momentum delivers them.
fn flick(engine: &mut Engine, view: EngineViewId, events: usize, step: f32) {
    let rt = runtime();
    for _ in 0..events {
        engine.scroll_view(view, 0.0, -step).unwrap();
        rt.block_on(engine.pump_live(view, 16));
    }
}

/// The window's frame at the view's offset, and the share of its pixels
/// that are not white.
fn inked(engine: &mut Engine, view: EngineViewId, tag: &str) -> f32 {
    let offset = engine.views[&view].scroll_offset;
    let viewhost_id = engine.views[&view].viewhost_id;
    let path = std::env::temp_dir().join(format!("rustkit-feed-{tag}-{}.ppm", std::process::id()));
    let list = engine.views[&view].display_list.clone().expect("display list");
    engine
        .presenter
        .as_mut()
        .expect("window presenter")
        .draw_scrolled(viewhost_id, &list, offset, Some(path.to_str().unwrap()))
        .expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    let pixels = (W * H) as usize;
    assert!(ppm.len() >= pixels * 3, "a {W}x{H} frame");
    let body = &ppm[ppm.len() - pixels * 3..];
    let inked = body.chunks(3).filter(|p| p.iter().any(|c| *c < 230)).count();
    inked as f32 / pixels as f32
}

/// Feed rows: 120 px tall, a dark band with a headline in it.
const ROW_CSS: &str = "body{margin:0;font:16px sans-serif} .row{height:120px;box-sizing:border-box;\
    border-bottom:4px solid #fff;background:#335;color:#fff} #top{height:200px;background:#c33} \
    #footer{height:80px;background:#3a3}";

fn shell(feed: &str, script: &str) -> String {
    format!(
        "<html><head><style>{ROW_CSS}</style></head><body><div id=top>masthead</div>\
         <div id=feed>{feed}</div><div id=footer>footer</div><script>{script}</script></body></html>"
    )
}

/// Builds `n` rows' markup in script.
const ROWS_JS: &str = "function rows(n, cls){var h='';for(var i=0;i<n;i++){h+='<div class=\"row '+(cls||'')+'\">headline '+i+'</div>';}return h;}";

/// Runs `pattern` twice: scrolled early (flicks right after load, before
/// the page's fill has run) and scrolled late (fill settled first). Both
/// frames must show the feed.
fn early_and_late(name: &str, settle_ms: u64, page: impl Fn() -> (String, Vec<(&'static str, &'static str, String)>)) {
    for early in [false, true] {
        let (html, extra) = page();
        let (mut engine, view, _server) = load(html, extra);
        if early {
            flick(&mut engine, view, 24, 40.0);
            settle(&mut engine, view, settle_ms);
        } else {
            settle(&mut engine, view, settle_ms);
            flick(&mut engine, view, 24, 40.0);
            settle(&mut engine, view, settle_ms);
        }
        let offset = engine.views[&view].scroll_offset;
        let max = engine.views[&view].max_scroll_offset;
        let share = inked(&mut engine, view, &format!("{name}-{early}"));
        let rows = read(&mut engine, view, "document.querySelectorAll('.row').length");
        eprintln!("{name} early={early}: offset={offset:?} max={max:?} rows={rows} inked={share:.3}");
        assert!(offset.1 > 600.0, "{name} early={early}: the flicks moved the page: {offset:?} of {max:?}");
        assert!(share > 0.5, "{name} early={early}: the feed paints at {offset:?}: {share:.3} inked, rows={rows}");
    }
}

#[test]
fn a_feed_a_timer_fills_paints_after_an_early_scroll() {
    early_and_late("timer", 6000, || {
        let script = format!("{ROWS_JS} setTimeout(function(){{document.getElementById('feed').innerHTML=rows(30);}}, 5300);");
        (shell("<div style='height:3600px'>loading</div>", &script), vec![])
    });
}

#[test]
fn a_feed_a_fetch_fills_paints_after_an_early_scroll() {
    early_and_late("fetch", 1500, || {
        let script = format!(
            "{ROWS_JS} window.addEventListener('load', function(){{ fetch('/feed').then(function(r){{return r.json();}})\
             .then(function(j){{document.getElementById('feed').innerHTML=rows(j.n);}}); }});"
        );
        (shell("<div class=row>first</div>", &script), vec![("/feed", "application/json", "{\"n\":30}".to_string())])
    });
}

#[test]
fn rows_an_observer_reveals_paint_after_an_early_scroll() {
    // Placeholders hold the rows' height; an IntersectionObserver fills
    // each as it nears the viewport.
    early_and_late("io", 1500, || {
        let script = "var io = new IntersectionObserver(function(es){es.forEach(function(e){if(e.isIntersecting){\
             e.target.className='row';e.target.textContent='headline';io.unobserve(e.target);}});},{rootMargin:'200px'});\
             document.querySelectorAll('.ph').forEach(function(p){io.observe(p);});";
        let feed = "<div class=ph style='height:120px'></div>".repeat(30);
        (shell(&feed, script), vec![])
    });
}

#[test]
fn rows_that_fade_in_on_reveal_paint_after_an_early_scroll() {
    // Server-rendered rows start at opacity 0 and get a class that fades
    // them in as they are revealed.
    early_and_late("fade", 1500, || {
        let script = "var io = new IntersectionObserver(function(es){es.forEach(function(e){if(e.isIntersecting){\
             e.target.classList.add('in');}});});\
             document.querySelectorAll('.row').forEach(function(p){io.observe(p);});";
        let feed = "<div class='row fade'>headline</div>".repeat(30);
        let html = shell(&feed, script).replace(
            "</style>",
            ".fade{opacity:0;transition:opacity .4s} .fade.in{opacity:1}</style>",
        );
        (html, vec![])
    });
}

#[test]
fn a_feed_a_scroll_listener_extends_paints_after_an_early_scroll() {
    // Infinite scroll: a scroll listener asks a frame to append a page of
    // rows when the user nears the bottom.
    early_and_late("scroll-raf", 1500, || {
        let script = format!(
            "{ROWS_JS} var busy=false; window.addEventListener('scroll', function(){{ if(busy) return; \
             if(window.scrollY + innerHeight > document.body.scrollHeight - 400){{ busy=true; \
             requestAnimationFrame(function(){{ document.getElementById('feed').insertAdjacentHTML('beforeend', rows(10)); busy=false; }}); }} }});"
        );
        (shell(&"<div class=row>x</div>".repeat(5), &script), vec![])
    });
}

/// 640 x 480 JPEG, a photo: dark enough to count as ink.
const JPEG: &[u8] = include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic4.jpg");

#[test]
fn lazy_images_an_observer_swaps_in_paint_after_a_scroll() {
    // Rows that are only a thumbnail: an IntersectionObserver copies
    // `data-src` to `src` as each nears the viewport. Nothing else on the
    // page runs once it is idle.
    for early in [false, true] {
        let script = "var io = new IntersectionObserver(function(es){es.forEach(function(e){if(e.isIntersecting){\
             e.target.src=e.target.dataset.src;io.unobserve(e.target);}});});\
             document.querySelectorAll('img.lazy').forEach(function(p){io.observe(p);});";
        let feed = (0..30)
            .map(|i| format!("<div class=row style='background:none'><img class=lazy data-src='/t{i}.jpg' style='display:block;width:400px;height:116px'></div>"))
            .collect::<String>();
        let html = shell(&feed, script);
        let names: Vec<&'static str> = (0..30).map(|i| &*Box::leak(format!("/t{i}.jpg").into_boxed_str())).collect();
        let mut routes = vec![("/", "text/html", html.into_bytes())];
        routes.extend(names.iter().map(|n| (*n, "image/jpeg", JPEG.to_vec())));
        let server = crate::script_net_tests::serve_byte_routes(routes);
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: W, height: H })
            .expect("view");
        let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
        runtime().block_on(engine.load_url(view, url)).expect("load_url");
        if early {
            flick(&mut engine, view, 2, 450.0);
            settle(&mut engine, view, 1500);
        } else {
            settle(&mut engine, view, 1500);
            flick(&mut engine, view, 2, 450.0);
            settle(&mut engine, view, 1500);
        }
        let offset = engine.views[&view].scroll_offset;
        let swapped = read(&mut engine, view, "Array.prototype.filter.call(document.querySelectorAll('img.lazy'), function(i){return !!i.getAttribute('src');}).length");
        let base = format!("http://127.0.0.1:{}", server.port);
        let cached = names
            .iter()
            .filter(|n| engine.lctx.image_manager.is_cached(&Url::parse(&format!("{base}{n}")).unwrap()))
            .count();
        let share = inked(&mut engine, view, &format!("lazy-img-{early}"));
        eprintln!("lazy-img early={early}: offset={offset:?} swapped={swapped} cached={cached} inked={share:.3}");
        assert!(offset.1 > 600.0, "the flicks moved the page: {offset:?}");
        assert_eq!(swapped.parse::<usize>().unwrap(), cached, "early={early}: every image the page asked for was fetched");
        assert!(share > 0.5, "early={early}: the thumbnails paint at {offset:?}: {share:.3} inked");
    }
}

#[test]
fn a_quiet_page_does_not_look_for_images_again() {
    // The plan above runs on a layout the live loop did not make. A page
    // that lays nothing out (a scroll alone does not) never asks again.
    let feed = "<div class=row><img src='/t.jpg' style='display:block;width:400px;height:116px'></div>".repeat(5);
    let routes = vec![
        ("/", "text/html", shell(&feed, "").into_bytes()),
        ("/t.jpg", "image/jpeg", JPEG.to_vec()),
    ];
    let server = crate::script_net_tests::serve_byte_routes(routes);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: W, height: H })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    runtime().block_on(engine.load_url(view, url)).expect("load_url");
    let planned = |e: &Engine| (e.views[&view].images_planned_for, e.views[&view].frame_generation);
    let (at, generation) = planned(&engine);
    assert_eq!(at, generation, "the load's own layouts count as looked at");

    settle(&mut engine, view, 200);
    flick(&mut engine, view, 2, 200.0);
    settle(&mut engine, view, 200);
    assert_eq!(planned(&engine), (generation, generation), "nothing laid out, nothing planned again");
}
