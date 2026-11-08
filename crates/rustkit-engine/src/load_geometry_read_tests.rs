//! Plan row CH2.E11: geometry read by a script during the load. The row's
//! claim was that such a read answers 0 until the engine's first layout
//! (Chart.js on report.html sizing its canvases 0x0). It does not: the
//! document is laid out before its first script runs (`load_html_as`,
//! `load_url`), and a write in the same task lays it out again on the read
//! (forced layout, `DomHost::force_layout_if_written`). These pin that, with
//! the strings Chromium 143.0.7499.4 gives for the same pages at 800x600.
//!
//! report.html's 0x0 canvases come from elsewhere: Chart.js measures its
//! container while `#content` is `display:none` (0 in Chromium too), and
//! resizes from a ResizeObserver callback through `requestAnimationFrame`.
//! That frame callback runs on the live loop's next turn, which the board's
//! `--html-file` capture never turns. The last test pins the pattern.

use super::*;
use crate::script_net_tests::serve_routes;

const WIDTH: u32 = 800;
const HEIGHT: u32 = 600;

fn view_of(engine: &mut Engine) -> EngineViewId {
    engine
        .create_headless_view(Bounds { x: 0, y: 0, width: WIDTH, height: HEIGHT })
        .expect("view")
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

/// What the page's script wrote into `#out`.
fn out(engine: &mut Engine, view: EngineViewId) -> String {
    let got = engine
        .execute_script(view, "document.getElementById('out').textContent")
        .expect("script");
    got.strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&got)
        .to_string()
}

fn load_html(page: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = view_of(&mut engine);
    engine.load_html(view, page).expect("load_html");
    (engine, view)
}

/// A sized box, then an inline script in `<body>` that reads it.
const INLINE_READ: &str = "<!doctype html><html><head><style>body{margin:0} \
    #box{width:300px;height:120px;padding:5px;border:2px solid red} #wrap{width:500px}</style></head>\
    <body><div id=wrap><div id=box></div></div><div id=out>x</div>\
    <script>\
    var b=document.getElementById('box'), r=b.getBoundingClientRect(), w=document.getElementById('wrap');\
    var v=['ow='+b.offsetWidth,'oh='+b.offsetHeight,'cw='+b.clientWidth,'ch='+b.clientHeight,\
      'sw='+b.scrollWidth,'gbcr='+r.width+'x'+r.height+'@'+r.x+','+r.y,'gcr='+b.getClientRects().length,\
      'cs='+getComputedStyle(b).width,'wrapcw='+w.clientWidth,'ot='+b.offsetTop,\
      'dw='+document.documentElement.clientWidth,'bodyoh='+document.body.offsetHeight];\
    document.getElementById('out').textContent='R:'+v.join(' ');\
    </script></body></html>";

const INLINE_READ_CHROMIUM: &str =
    "R:ow=314 oh=134 cw=310 ch=130 sw=310 gbcr=314x134@0,0 gcr=1 cs=300px wrapcw=500 ot=0 dw=800 bodyoh=152";

#[test]
fn an_inline_script_reads_the_laid_out_box_in_an_html_load() {
    let (mut engine, view) = load_html(INLINE_READ);
    assert_eq!(out(&mut engine, view), INLINE_READ_CHROMIUM);
}

#[test]
fn an_inline_script_reads_the_laid_out_box_in_a_url_load() {
    let server = serve_routes(vec![("/", "text/html", INLINE_READ.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = view_of(&mut engine);
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    runtime().block_on(engine.load_url(view, url)).expect("load_url");
    assert_eq!(out(&mut engine, view), INLINE_READ_CHROMIUM);
}

#[test]
fn a_script_after_a_linked_sheet_reads_the_box_the_sheet_sized() {
    // Chromium holds a parser-blocking script until the sheets before it
    // have loaded, so the read sees the sheet's 250px (+3px padding).
    let page = "<!doctype html><html><head><style>body{margin:0}</style>\
        <link rel=stylesheet href=sheet.css></head><body><div id=box></div><div id=out>x</div>\
        <script>var b=document.getElementById('box');\
        document.getElementById('out').textContent='R:'+[b.offsetWidth,b.offsetHeight,b.clientWidth,\
          b.getBoundingClientRect().height,getComputedStyle(b).width].join(' ');</script></body></html>";
    let server = serve_routes(vec![
        ("/", "text/html", page.to_string()),
        ("/sheet.css", "text/css", "#box{width:250px;height:40px;padding:3px}".to_string()),
    ]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = view_of(&mut engine);
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    runtime().block_on(engine.load_url(view, url)).expect("load_url");
    assert_eq!(out(&mut engine, view), "R:256 46 256 46 250px");
}

#[test]
fn a_resize_observer_callback_writes_and_reads_once_without_recursing() {
    // The callback writes the box's width and reads it back (a forced layout
    // inside the observer delivery); the observed #wrap keeps its size, so
    // it is delivered once.
    let page = "<!doctype html><html><head><style>body{margin:0} #wrap{width:500px} \
        #box{width:300px;height:20px}</style></head><body><div id=wrap><div id=box></div></div>\
        <div id=out>x</div><script>\
        var n=0,box=document.getElementById('box');\
        new ResizeObserver(function(es){n++;var a=box.offsetWidth;box.style.width='200px';\
          var b=box.offsetWidth,r=box.getBoundingClientRect();\
          document.getElementById('out').textContent='R:'+[n,es[0].contentRect.width,a,b,r.width,\
            document.getElementById('wrap').clientHeight].join(' ');})\
          .observe(document.getElementById('wrap'));\
        </script></body></html>";
    let (mut engine, view) = load_html(page);
    let rt = runtime();
    for _ in 0..4 {
        rt.block_on(engine.pump_live(view, 16));
    }
    assert_eq!(out(&mut engine, view), "R:1 500 300 200 200 20");
}

#[test]
fn a_chart_sized_from_a_resize_observer_frame_callback_gets_its_container_on_the_next_turn() {
    // report.html's shape: the content is hidden while the chart is built
    // (0x0 in Chromium too), shown in the same task, and the chart resizes
    // from ResizeObserver through requestAnimationFrame (Chart.js 4.4's
    // `throttled`).
    let page = "<!doctype html><html><head><style>body{margin:0} .cc{position:relative;height:300px} \
        #content{padding:10px}</style></head><body><div id=loading>Loading</div>\
        <div id=content style='display:none'><div class=cc id=cc><canvas id=c></canvas></div></div>\
        <div id=out>x</div><script>\
        var log=[];function L(s){log.push(s);document.getElementById('out').textContent='R:'+log.join(' ');}\
        var cc=document.getElementById('cc'),c=document.getElementById('c');\
        L('parse='+cc.clientWidth+'x'+cc.clientHeight);\
        document.addEventListener('DOMContentLoaded',function(){\
          document.getElementById('loading').style.display='block';\
          document.getElementById('content').style.display='none';\
          Promise.resolve().then(function(){\
            L('hidden='+cc.clientWidth+'x'+cc.clientHeight+'/'+Math.round(cc.getBoundingClientRect().width));\
            new ResizeObserver(function(es){var r=es[0].contentRect;L('ro='+r.width+'x'+r.height);\
              requestAnimationFrame(function(){c.width=r.width;c.height=r.height;\
                c.style.width=r.width+'px';c.style.height=r.height+'px';\
                L('raf='+c.offsetWidth+'x'+c.offsetHeight);});\
            }).observe(cc);\
            document.getElementById('loading').style.display='none';\
            document.getElementById('content').style.display='block';\
            L('shown='+cc.clientWidth+'x'+cc.clientHeight);\
          });\
        });\
        </script></body></html>";
    let (mut engine, view) = load_html(page);
    // The load itself: the observer has delivered, its frame callback waits
    // for the live loop (the board's capture stops here).
    assert_eq!(
        out(&mut engine, view),
        "R:parse=0x0 hidden=0x0/0 shown=780x300 ro=780x300"
    );
    let rt = runtime();
    for _ in 0..4 {
        rt.block_on(engine.pump_live(view, 16));
    }
    assert_eq!(
        out(&mut engine, view),
        "R:parse=0x0 hidden=0x0/0 shown=780x300 ro=780x300 raf=780x300",
        "Chromium 143 at 800x600"
    );
}
