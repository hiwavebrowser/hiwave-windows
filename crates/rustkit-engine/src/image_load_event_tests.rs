//! E54: an `<img>` hears `load`, and script can read that it loaded.
//!
//! Found by the Chrome probe of LAT9-r1: the engine never fired `load` at an
//! image, and one that had loaded read `complete === false` and
//! `naturalWidth === 0`. Lazy-load libraries, carousels, fade-in-on-load
//! hooks and "wait for the images, then measure" code wait on exactly
//! these, so their images stayed hidden or unmeasured.
//!
//! The expectations are Chrome for Testing 148.0.7778.216 on the pages of
//! `tools/parity_oracle/img_load_log.mjs` (headless, loopback): `load` fires
//! once per successful load, at the element only (it does not bubble), for
//! an attribute handler, a property handler and a listener alike; inside
//! it and from then on `complete` is true, `naturalWidth`/`naturalHeight`
//! are the image's own size and `currentSrc` the URL that loaded; the
//! window's `load` comes after the `load` or `error` of every image that
//! was in the document, and does not wait for one the engine gave up on.
//!
//! Each page logs to `L` what it heard and what the image read then:
//! `{complete,naturalWidth,naturalHeight,currentSrc}`.

use super::*;
use crate::script_net_tests::{serve_byte_routes, Server};

/// 40 x 30.
const A: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 40, 0, 0, 0, 30, 8, 2, 0, 0, 0, 209, 191, 203, 138, 0, 0, 0, 40, 73, 68, 65, 84, 120, 156, 237, 205, 49, 1, 0, 0, 12, 2, 32, 211, 218, 63, 130, 49, 182, 3, 10, 144, 30, 137, 88, 44, 22, 139, 197, 98, 177, 88, 44, 22, 139, 255, 198, 3, 224, 195, 154, 172, 7, 14, 39, 42, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];
/// 60 x 20.
const B: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 60, 0, 0, 0, 20, 8, 2, 0, 0, 0, 94, 97, 201, 126, 0, 0, 0, 50, 73, 68, 65, 84, 120, 156, 237, 206, 65, 13, 0, 32, 12, 0, 177, 169, 197, 191, 4, 60, 236, 81, 66, 114, 85, 208, 57, 31, 154, 215, 129, 141, 210, 74, 105, 165, 180, 82, 90, 41, 173, 148, 86, 74, 43, 165, 149, 210, 202, 5, 219, 53, 154, 172, 3, 176, 106, 158, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];
const A_DATA: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAeCAIAAADRv8uKAAAAKElEQVR4nO3NMQEAAAwCINPaP4IxtgMKkB6JWCwWi8VisVgsFov/xgPgw5qsBw4nKgAAAABJRU5ErkJggg==";
const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="12"><rect width="24" height="12"/></svg>"#;

const HARNESS: &str = "<script>var L=[];\
    function st(i){return '{'+[i.complete,i.naturalWidth,i.naturalHeight,(i.currentSrc||'').replace(location.origin,'').slice(0,24)].join(',')+'}';}\
    function ev(t,i){L.push(t+(i?st(i):''));}\
    function $(id){return document.getElementById(id);}\
    function watch(i,n){['load','error'].forEach(function(t){i.addEventListener(t,function(){ev(n+'.'+t,i);});});return i;}\
    document.addEventListener('DOMContentLoaded',function(){ev('DCL');});\
    window.addEventListener('load',function(){ev('winload');});</script>";

fn a(path: &'static str) -> (&'static str, &'static str, Vec<u8>) {
    (path, "image/png", A.to_vec())
}

fn b(path: &'static str) -> (&'static str, &'static str, Vec<u8>) {
    (path, "image/png", B.to_vec())
}

fn runtime() -> &'static tokio::runtime::Runtime {
    thread_local! {
        static RT: &'static tokio::runtime::Runtime = Box::leak(Box::new(
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(),
        ));
    }
    RT.with(|rt| *rt)
}

/// How the page is loaded: as the app loads it (what the page asks of the
/// network after its `load` is the live loop's), or as a capture does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    App,
    Capture,
}

const BOTH: [Mode; 2] = [Mode::App, Mode::Capture];

struct Page {
    engine: Engine,
    view: EngineViewId,
    server: Server,
    took: std::time::Duration,
}

fn load(body: &str, routes: Vec<(&'static str, &'static str, Vec<u8>)>, mode: Mode, budget_ms: u64) -> Page {
    let config = EngineConfig {
        subresource_budget_ms: budget_ms,
        post_load_requests_live: mode == Mode::App,
        ..EngineConfig::default()
    };
    load_with(body, routes, config)
}

fn load_with(body: &str, mut routes: Vec<(&'static str, &'static str, Vec<u8>)>, config: EngineConfig) -> Page {
    let page = format!("<html><head>{HARNESS}</head><body>{body}</body></html>");
    routes.insert(0, ("/", "text/html", page.into_bytes()));
    let server = serve_byte_routes(routes);
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let started = std::time::Instant::now();
    runtime().block_on(engine.load_url(view, url)).expect("load_url");
    let took = started.elapsed();
    let mut page = Page { engine, view, server, took };
    page.settle();
    page
}

impl Page {
    /// Live turns until nothing is in flight, and a few more.
    fn settle(&mut self) {
        let rt = runtime();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        rt.block_on(self.engine.pump_live(self.view, 16));
        while std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let turn = rt.block_on(self.engine.pump_live(self.view, 16));
            if turn.in_flight == 0 && !turn.yielded_for_frame {
                break;
            }
        }
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            rt.block_on(self.engine.pump_live(self.view, 16));
        }
    }

    /// One live turn, `ms` from now.
    fn turn_after(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        runtime().block_on(self.engine.pump_live(self.view, 16));
    }

    fn run(&mut self, script: &str) -> String {
        self.engine
            .execute_script(self.view, script)
            .unwrap()
            .trim_start_matches("String(\"")
            .trim_end_matches("\")")
            .to_string()
    }

    /// What the page logged.
    fn log(&mut self) -> String {
        self.run("L.join(' | ')")
    }

    fn painted(&self) -> Vec<String> {
        self.engine.views[&self.view]
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
}

/// An image in the markup: `load` reaches an `onload` attribute, an
/// `onload` property and a listener, once each, at the element only. A
/// capturing listener on the document sees it; a bubbling one does not.
/// Inside the handler the image reads loaded, and all of it comes after
/// DOMContentLoaded and before the window's `load`.
#[test]
fn a_markup_image_hears_load_once_by_attribute_property_and_listener() {
    for mode in BOTH {
        let body = "<img id=a src=/a.png?delay=150 onload=\"ev('a.attr',this)\"><img id=b src=/b.png><img id=c src=/c.png>\
            <script>$('b').onload=function(){ev('b.prop',this)};watch($('c'),'c');\
            document.addEventListener('load',function(e){ev('doc.capture:'+e.target.id)},true);\
            document.addEventListener('load',function(e){ev('doc.bubble:'+e.target.id)});</script>";
        let mut page = load(body, vec![a("/a.png?delay=150"), b("/b.png"), a("/c.png")], mode, 5_000);
        assert_eq!(
            page.log(),
            "DCL | doc.capture:a | a.attr{true,40,30,/a.png?delay=150} | doc.capture:b | b.prop{true,60,20,/b.png} \
             | doc.capture:c | c.load{true,40,30,/c.png} | winload",
            "{mode:?}"
        );
        assert_eq!(page.run("st($('a'))+st($('b'))"), "{true,40,30,/a.png?delay=150}{true,60,20,/b.png}", "{mode:?}: afterwards");
    }
}

/// Listeners attached at DOMContentLoaded still hear the image, as they do
/// in Chrome for an image that is slower than the parse.
#[test]
fn a_listener_added_at_domcontentloaded_hears_the_image() {
    for mode in BOTH {
        let body = "<img id=a src=/a.png><script>\
            document.addEventListener('DOMContentLoaded',function(){ev('at-dcl',$('a'));watch($('a'),'a');});</script>";
        let mut page = load(body, vec![a("/a.png")], mode, 5_000);
        assert_eq!(page.log(), "DCL | at-dcl{false,0,0,} | a.load{true,40,30,/a.png} | winload", "{mode:?}");
    }
}

/// The event is Chrome's: a trusted `Event` that neither bubbles nor can be
/// cancelled, with the image as `this`. So is `error`.
#[test]
fn the_load_event_is_a_trusted_event_that_does_not_bubble() {
    let body = "<img id=a src=/a.png><img id=x src=/missing.png><script>\
        function shape(e){ev(e.type+' target='+e.target.id+' bubbles='+e.bubbles+' cancelable='+e.cancelable\
        +' trusted='+e.isTrusted+' ctor='+e.constructor.name+' this='+(this===e.target));}\
        $('a').addEventListener('load',shape);$('x').addEventListener('error',shape);</script>";
    let mut page = load(body, vec![a("/a.png")], Mode::App, 5_000);
    let log = page.log();
    assert!(log.contains("load target=a bubbles=false cancelable=false trusted=true ctor=Event this=true"), "{log}");
    assert!(log.contains("error target=x bubbles=false cancelable=false trusted=true ctor=Event this=true"), "{log}");
}

/// The window's `load` waits for an image a script put in the document
/// before it (here at DOMContentLoaded), as the app loads a page.
#[test]
fn the_window_load_waits_for_an_image_a_script_inserted() {
    let body = "<div id=box></div><script>document.addEventListener('DOMContentLoaded',function(){\
        var i=watch(document.createElement('img'),'a');i.id='a';i.src='/a.png?delay=300';$('box').appendChild(i);});</script>";
    let mut page = load(body, vec![a("/a.png?delay=300")], Mode::App, 5_000);
    assert_eq!(page.log(), "DCL | a.load{true,40,30,/a.png?delay=300} | winload");
}

/// A load that yields while a script it inserted is still out (the app's
/// load) fires the window's `load` from a live turn: the image has heard
/// its own by then.
#[test]
fn a_yielded_load_tells_the_image_before_the_window_load() {
    let body = "<img id=a src=/a.png><script>watch($('a'),'a');var s=document.createElement('script');\
        s.src='/late.js?delay=200';document.head.appendChild(s);</script>";
    let page = format!("<html><head>{HARNESS}</head><body>{body}</body></html>");
    let server = serve_byte_routes(vec![
        ("/", "text/html", page.into_bytes()),
        a("/a.png"),
        ("/late.js?delay=200", "text/javascript", b"ev('late');".to_vec()),
    ]);
    let config = EngineConfig { post_load_requests_live: true, ..EngineConfig::default() };
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let yielded = runtime().block_on(engine.load_url_yielding(view, url)).expect("load");
    assert!(matches!(yielded, LoadYield::Yielded), "precondition: the load yielded");
    let mut page = Page { engine, view, server, took: Default::default() };
    page.settle();
    page.turn_after(300);
    page.settle();
    let log = page.log();
    assert_eq!(log.matches("a.load{true,40,30,/a.png}").count(), 1, "{log}");
    assert!(log.ends_with("| winload"), "{log}");
    assert!(log.contains("late"), "{log}");
}

/// A broken image's `error` also comes before the window's `load` (LAT9-r1
/// fired it on the first live turn after).
#[test]
fn a_broken_image_hears_error_before_the_window_load() {
    for mode in BOTH {
        let body = "<img id=a src=/missing.png><img id=b src=/zero.png><img id=c src=/garbage.png>\
            <script>['a','b','c'].forEach(function(n){watch($(n),n)});</script>";
        let routes = vec![("/zero.png", "image/png", Vec::new()), ("/garbage.png", "image/png", b"not an image".to_vec())];
        let mut page = load(body, routes, mode, 5_000);
        assert_eq!(
            page.log(),
            "DCL | a.error{true,0,0,/missing.png} | b.error{true,0,0,/zero.png} | c.error{true,0,0,/garbage.png} | winload",
            "{mode:?}"
        );
    }
}

/// An image the engine gave up on (its budget ran out) does not hold the
/// window's `load`: the load waits one subresource budget for the images
/// of the document, then goes on. The image reads as still loading and
/// hears nothing.
#[test]
fn an_image_the_engine_gave_up_on_does_not_hold_the_window_load() {
    for mode in BOTH {
        let body = "<img id=a src=/hang.png?delay=4000><script>watch($('a'),'a');</script>";
        let mut page = load(body, vec![a("/hang.png?delay=4000")], mode, 400);
        assert!(page.took < std::time::Duration::from_millis(2_500), "{mode:?}: the load took {:?}", page.took);
        assert_eq!(page.log(), "DCL | winload", "{mode:?}");
        assert_eq!(page.run("st($('a'))"), "{false,0,0,}", "{mode:?}");
    }
}

/// An image given its `src` after the load: nothing until it is in, then
/// `load`.
#[test]
fn an_image_given_its_src_after_the_load_hears_load() {
    for mode in BOTH {
        let mut page = load("<img id=a>", vec![a("/a.png?delay=300")], mode, 5_000);
        assert_eq!(page.run("st($('a'))"), "{true,0,0,}", "{mode:?}: no src is complete");
        page.run("watch($('a'),'a');$('a').src='/a.png?delay=300';ev('set',$('a'));0");
        page.turn_after(50);
        assert_eq!(page.log(), "DCL | winload | set{false,0,0,}", "{mode:?}: in flight");
        assert_eq!(page.run("st($('a'))"), "{false,0,0,}", "{mode:?}: in flight");
        page.settle();
        assert_eq!(page.log(), "DCL | winload | set{false,0,0,} | a.load{true,40,30,/a.png?delay=300}", "{mode:?}");
    }
}

/// An image a script adds after the load.
#[test]
fn an_image_added_after_the_load_hears_load() {
    let mut page = load("<div id=box></div>", vec![a("/a.png")], Mode::App, 5_000);
    page.run("var i=watch(document.createElement('img'),'a');i.id='a';i.src='/a.png';$('box').appendChild(i);0");
    page.settle();
    assert_eq!(page.log(), "DCL | winload | a.load{true,40,30,/a.png}");
}

/// Image handlers are the live turn's script, charged to its budget of
/// JavaScript time: once that is spent the turn fires no more, says it has
/// more, and the next turn goes on. Each element still hears once.
#[test]
fn image_handlers_past_the_turn_budget_wait_for_the_next_turn() {
    let config = EngineConfig { post_load_requests_live: true, live_turn_budget_ms: 60, ..EngineConfig::default() };
    let mut page = load_with("<img id=a src=/a.png><div id=box></div>", vec![a("/a.png")], config);
    page.run(
        "for(var k=0;k<6;k++){var i=document.createElement('img');i.onload=function(){\
         var t=Date.now(),n=0;while(Date.now()-t<40&&n<200000)n++;ev('slow');};i.src='/a.png';$('box').appendChild(i);}0",
    );
    let turn = runtime().block_on(page.engine.pump_live(page.view, 16));
    let first = page.log().matches("slow").count();
    assert!((1..6).contains(&first), "the first turn fired some and left some: {first}");
    assert_eq!(turn.next_timer_ms, Some(0), "the rest are due at once");
    page.settle();
    assert_eq!(page.log().matches("slow").count(), 6, "{}", page.log());
}

/// A good image swapped for another good one: `load` again, with the new
/// image's size. While the new one is in flight the element is not
/// complete.
#[test]
fn a_src_swap_hears_load_for_each_image() {
    let body = "<img id=a src=/a.png><script>watch($('a'),'a');</script>";
    let mut page = load(body, vec![a("/a.png"), b("/b.png?delay=300")], Mode::App, 5_000);
    page.run("$('a').src='/b.png?delay=300';0");
    page.turn_after(50);
    assert_eq!(page.run("st($('a'))"), "{false,0,0,}", "in flight");
    page.settle();
    assert_eq!(
        page.log(),
        "DCL | a.load{true,40,30,/a.png} | winload | a.load{true,60,20,/b.png?delay=300}"
    );
}

/// The same `src` written again loads again: the element stays complete
/// and hears a second `load`, by the property and by `setAttribute`.
#[test]
fn the_same_src_written_again_hears_load_again() {
    for write in ["$('a').src='/a.png'", "$('a').setAttribute('src','/a.png')"] {
        let body = "<img id=a src=/a.png><script>watch($('a'),'a');</script>";
        let mut page = load(body, vec![a("/a.png")], Mode::App, 5_000);
        page.run(&format!("{write};ev('set',$('a'));0"));
        page.settle();
        assert_eq!(
            page.log(),
            "DCL | a.load{true,40,30,/a.png} | winload | set{true,40,30,/a.png} | a.load{true,40,30,/a.png}",
            "{write}"
        );
        page.settle();
        assert_eq!(page.log().matches("a.load").count(), 2, "{write}: and no more");
    }
}

/// Two elements showing one URL each hear `load`; so does a third that
/// arrives once the image is cached. One request serves them all.
#[test]
fn every_element_showing_one_url_hears_load() {
    let body = "<img id=a src=/a.png><img id=b src=/a.png><div id=box></div><script>watch($('a'),'a');watch($('b'),'b');</script>";
    let mut page = load(body, vec![a("/a.png")], Mode::App, 5_000);
    assert_eq!(page.log(), "DCL | a.load{true,40,30,/a.png} | b.load{true,40,30,/a.png} | winload");
    page.run("var c=watch(document.createElement('img'),'c');c.src='/a.png';$('box').appendChild(c);0");
    page.settle();
    assert!(page.log().ends_with("winload | c.load{true,40,30,/a.png}"), "{}", page.log());
    assert_eq!(page.server.served("/a.png").len(), 1, "one request");
}

/// `currentSrc` is the URL that loaded: with `srcset`, the candidate the
/// engine chose (the one it paints), not `src`.
#[test]
fn current_src_is_the_srcset_candidate_that_loaded() {
    let body = "<img id=a src=/c.png srcset=\"/a.png 1x, /b.png 2x\"><script>watch($('a'),'a');</script>";
    let mut page = load(body, vec![a("/a.png"), b("/b.png"), a("/c.png")], Mode::App, 5_000);
    let painted = page.painted();
    assert_eq!(painted.len(), 1);
    let chosen = painted[0].rsplit('/').next().unwrap().to_string();
    let size = if chosen == "b.png" { "60,20" } else { "40,30" };
    assert_eq!(page.log(), format!("DCL | a.load{{true,{size},/{chosen}}} | winload"));
    assert!(page.server.served("/c.png").is_empty(), "`src` is not fetched beside the srcset");
}

/// An SVG image and a `data:` URL: `load`, with the SVG's own width and
/// height as its natural size.
#[test]
fn svg_and_data_url_images_hear_load() {
    let body = format!(
        "<img id=a src=/v.svg><img id=d src=\"{A_DATA}\"><script>watch($('a'),'a');watch($('d'),'d');</script>"
    );
    let mut page = load(&body, vec![("/v.svg", "image/svg+xml", SVG.as_bytes().to_vec())], Mode::App, 5_000);
    assert_eq!(
        page.log(),
        "DCL | a.load{true,24,12,/v.svg} | d.load{true,40,30,data:image/png;base64,iV} | winload"
    );
}

/// An image that is not displayed still loads and hears `load`.
#[test]
fn a_display_none_image_hears_load() {
    let body = "<img id=a src=/a.png style=\"display:none\"><script>watch($('a'),'a');</script>";
    let mut page = load(body, vec![a("/a.png")], Mode::App, 5_000);
    assert_eq!(page.log(), "DCL | a.load{true,40,30,/a.png} | winload");
}

/// "Wait for the image, then measure": in the handler the box already has
/// the image's size.
#[test]
fn the_load_handler_measures_the_image() {
    let body = "<img id=a src=/a.png><script>$('a').addEventListener('load',function(){\
        var r=this.getBoundingClientRect();ev('box='+r.width+'x'+r.height+' wh='+this.width+'x'+this.height);});</script>";
    let mut page = load(body, vec![a("/a.png")], Mode::App, 5_000);
    assert_eq!(page.log(), "DCL | box=40x30 wh=40x30 | winload");
}

/// A load handler that writes the image's style and reads `img.width`
/// (a page's "is this the 80 x 80 placeholder" check, run for every result
/// card of a recorded search page) must not lay the page out for it: with
/// no `width`/`height` attribute the engine answers with the image's own
/// size. Chrome lays out and answers the rendered size, which is the same
/// for an image CSS does not size; the engine's forced layout of a large
/// page costs it far more than Chrome's.
#[test]
fn reading_img_width_in_a_load_handler_does_not_force_a_layout() {
    let body = "<img id=a src=/a.png><img id=b src=/b.png><script>[$('a'),$('b')].forEach(function(i){i.addEventListener('load',function(){\
        this.setAttribute('style','width:auto');ev(this.id+'='+this.width+'x'+this.height);});});</script>";
    for mode in BOTH {
        let mut page = load(body, vec![a("/a.png"), b("/b.png")], mode, 5_000);
        assert_eq!(page.log(), "DCL | a=40x30 | b=60x20 | winload", "{mode:?}");
        let forced = page.engine.relayout_stats(page.view).expect("view").forced_layouts;
        assert_eq!(forced, 0, "{mode:?}: the handlers laid the page out {forced} times");
    }
}

/// An image that never enters the document (`new Image()`, a created
/// element) is fetched and hears `load` or `error`: the preloader and the
/// pixel. It is not painted.
#[test]
fn an_image_that_is_never_inserted_loads_and_hears_load() {
    let body = "<script>var i=new Image();i.onload=function(){ev('new.onload',i)};i.onerror=function(){ev('new.onerror',i)};\
        i.src='/a.png?delay=100';ev('set',i);\
        var j=watch(document.createElement('img'),'created');j.src='/b.png';\
        var k=watch(new Image(),'broken');k.src='/missing.png';</script>";
    for mode in BOTH {
        let mut page = load(body, vec![a("/a.png?delay=100"), b("/b.png")], mode, 5_000);
        let log = page.log();
        assert!(log.starts_with("set{false,0,0,} | DCL | "), "{mode:?}: {log}");
        for heard in ["new.onload{true,40,30,/a.png?delay=100}", "created.load{true,60,20,/b.png}", "broken.error{true,0,0,/missing.png}"] {
            assert_eq!(log.matches(heard).count(), 1, "{mode:?}: {heard} once in {log}");
        }
        assert!(!log.contains("new.onerror"), "{mode:?}: {log}");
        assert_eq!(page.server.served("/a.png?delay=100").len(), 1, "{mode:?}");
        assert!(page.painted().is_empty(), "{mode:?}: nothing of it is painted");
        page.settle();
        assert_eq!(page.log(), log, "{mode:?}: each once");
    }
}

/// Added after the load, as a lazy-loader does: preload off the document,
/// and act in `onload`.
#[test]
fn a_preloader_made_after_the_load_hears_load() {
    let mut page = load("<img id=a>", vec![a("/a.png?delay=100")], Mode::App, 5_000);
    page.run("var p=new Image();p.onload=function(){ev('preloaded',p);$('a').src=p.src;};p.src='/a.png?delay=100';watch($('a'),'a');0");
    page.settle();
    assert_eq!(
        page.log(),
        "DCL | winload | preloaded{true,40,30,/a.png?delay=100} | a.load{true,40,30,/a.png?delay=100}"
    );
    assert_eq!(page.server.served("/a.png?delay=100").len(), 1, "the preload is the load");
}

/// `decode()` resolves once the image is in (before or after it is called)
/// and rejects with an `EncodingError` for an image that failed.
#[test]
fn decode_settles_with_the_image() {
    let body = "<img id=a src=/a.png?delay=150><img id=x src=/missing.png><script>var a=$('a'),x=$('x');\
        function d(i,name){i.decode().then(function(v){ev(name+'.resolved:'+v,i)},function(e){ev(name+'.rejected:'+(e&&e.name),i)});}\
        d(a,'early');d(x,'broken');a.addEventListener('load',function(){ev('a.load');d(a,'inload');});\
        var n=new Image();n.src='/b.png';d(n,'new');</script>";
    let mut page = load(body, vec![a("/a.png?delay=150"), b("/b.png")], Mode::App, 5_000);
    page.run("d(a,'late');0");
    page.settle();
    let log = page.log();
    for settled in [
        "early.resolved:undefined{true,40,30,/a.png?delay=150}",
        "inload.resolved:undefined{true,40,30,/a.png?delay=150}",
        "late.resolved:undefined{true,40,30,/a.png?delay=150}",
        "new.resolved:undefined{true,60,20,/b.png}",
        "broken.rejected:EncodingError{true,0,0,/missing.png}",
    ] {
        assert_eq!(log.matches(settled).count(), 1, "{settled} once in {log}");
    }
    let at = |what: &str| log.find(what).unwrap_or_else(|| panic!("{what} in {log}"));
    assert!(at("a.load") < at("early.resolved"), "decode() resolves with the load, not before: {log}");
    assert!(at("a.load") < at("winload"), "{log}");
}

/// R1 nit 1: an image the engine gave up on (its budget spent) hears no
/// event, so a `decode()` waiting on it would wait for good and content
/// revealed after `await img.decode()` would stay hidden. Its pending
/// `decode()` resolves when the engine gives up, as it did before E54, and
/// one asked for afterwards resolves at once.
#[test]
fn decode_settles_when_the_engine_gives_up_on_the_image() {
    let body = "<img id=a src=/hang.png?delay=4000><script>\
        $('a').decode().then(function(){ev('early.resolved')},function(e){ev('early.rejected:'+e.name)});</script>";
    let mut page = load(body, vec![a("/hang.png?delay=4000")], Mode::App, 400);
    page.settle();
    page.run("$('a').decode().then(function(){ev('late.resolved')},function(e){ev('late.rejected:'+e.name)});0");
    page.settle();
    let log = page.log();
    assert!(log.contains("early.resolved") && log.contains("late.resolved"), "{log}");
    assert!(!log.contains("a.load") && !log.contains("error"), "{log}");
}

/// Chrome rejects a pending `decode()` with an EncodingError when `src`
/// changes before the image is in.
#[test]
fn a_src_change_rejects_a_pending_decode() {
    let mut page = load("<img id=a>", vec![a("/a.png?delay=300"), b("/b.png")], Mode::App, 5_000);
    page.run("var i=$('a');i.src='/a.png?delay=300';i.decode().then(function(){ev('resolved')},function(e){ev('rejected:'+e.name)});i.src='/b.png';0");
    page.settle();
    let log = page.log();
    assert!(log.contains("rejected:EncodingError") && !log.contains("resolved"), "{log}");
}

/// R1 nit 5: writing the same `src` after a failed load starts the load
/// over, so the element reads as in flight until it settles again, not as
/// the failure it had.
#[test]
fn a_same_src_rewrite_after_a_failure_reads_as_in_flight() {
    let body = "<img id=a src=/missing.png?delay=200><script>watch($('a'),'a');</script>";
    let mut page = load(body, vec![], Mode::App, 5_000);
    assert!(page.log().contains("a.error{true,0,0,/missing.png?delay=200}"), "{}", page.log());
    page.run("$('a').setAttribute('src','/missing.png?delay=200');ev('set',$('a'));0");
    page.settle();
    let log = page.log();
    assert!(log.contains("set{false,0,0,}"), "in flight after the write: {log}");
    assert_eq!(log.matches("a.error{true,0,0,/missing.png?delay=200}").count(), 2, "{log}");
}

/// R1 nit 3: an `<img>` in a template's contents is in an inert document;
/// Chrome does not fetch it, and it hears nothing.
#[test]
fn an_img_in_template_content_is_not_fetched() {
    let body = "<template id=t><img id=x></template><script>\
        var i=$('t').content.querySelector('img');watch(i,'tpl');i.src='/t.png';</script>";
    let mut page = load(body, vec![a("/t.png")], Mode::App, 5_000);
    page.settle();
    assert!(page.server.served("/t.png").is_empty(), "the template's image was fetched");
    assert!(!page.log().contains("tpl."), "{}", page.log());
}

/// R1 nit 4: a `data:` URL that does not decode is a failed load: `error`.
#[test]
fn a_broken_data_url_image_hears_error() {
    let body = "<img id=a src=\"data:image/png;base64,AAAAAAAA\"><script>watch($('a'),'a');</script>";
    let mut page = load(body, vec![], Mode::App, 5_000);
    let log = page.log();
    assert!(log.contains("a.error{true,0,0,") && !log.contains("a.load"), "{log}");
}

/// R1 nit 2: an image outside the document is dropped from the engine's
/// list once the engine gave up on it, or once the node is gone.
#[test]
fn the_detached_list_is_pruned_after_a_give_up() {
    let mut page = load("", vec![a("/hang.png?delay=4000")], Mode::App, 300);
    page.run("var p=new Image();p.src='/hang.png?delay=4000';0");
    page.settle();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    page.settle();
    let left = page.engine.views[&page.view].lay.loaded_images.borrow().detached.len();
    assert_eq!(left, 0, "detached entries left after the give-up");
}

/// Prometheus on #1039 (LOW): an image that never enters the document picks its source by the
/// same E54b rule as one in the document, so `new Image()` with a `srcset` at DPR 2 fetches the 2x
/// candidate and never the 1x (detached_img_elements goes through pick_image_source with the
/// view's Env, not the old first-candidate pick).
#[test]
fn a_detached_image_picks_its_srcset_candidate_for_the_dpr() {
    let config = EngineConfig { device_pixel_ratio: 2.0, subresource_budget_ms: 5_000, ..EngineConfig::default() };
    let mut page = load_with("", vec![a("/one.png"), b("/two.png")], config);
    page.run("var p=new Image();p.srcset='/one.png 1x, /two.png 2x';0");
    page.settle();
    assert_eq!(page.server.served("/two.png").len(), 1, "the 2x candidate is fetched at DPR 2");
    assert!(page.server.served("/one.png").is_empty(), "and the 1x one is not");
}
