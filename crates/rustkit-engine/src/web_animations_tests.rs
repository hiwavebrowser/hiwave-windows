//! Web Animations `Element.animate()` (S8). A site ships its hero at
//! `style="opacity:0"` and fades it in with
//! `el.animate([{opacity:0},{opacity:1}],200).finished.then(() => el.style.opacity = "")`.
//! With no `animate`, the call threw and the hero stayed transparent: laid
//! out, never painted. These pages are that shape, with placeholder text.
//!
//! The clock is the page's timer clock (animation frames on the virtual
//! timeline the engine advances), so `pump` moves the animation as it moves
//! `requestAnimationFrame`.

use super::*;
use crate::script_net_tests::serve_routes;
use rustkit_layout::DisplayCommand;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn load(page: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn run(engine: &mut Engine, view: EngineViewId, script: &str) {
    engine.execute_script(view, script).unwrap();
}

/// Live turns of `step` ms until `total` ms have passed (the window loop).
fn pump(engine: &mut Engine, view: EngineViewId, total: u64, step: u64) {
    let rt = rt();
    let mut t = 0;
    while t < total {
        rt.block_on(engine.pump_live(view, step));
        t += step;
    }
}

fn opacity(engine: &mut Engine, view: EngineViewId) -> f64 {
    let s = read(engine, view, "getComputedStyle(document.getElementById('h')).opacity");
    s.parse().unwrap_or_else(|_| panic!("computed opacity {s:?}"))
}

/// Whether the hero's text is in the display list outside any opacity group
/// below 1 (the page has no other translucent box).
fn hero_painted(engine: &Engine, view: EngineViewId) -> bool {
    let list = engine.views[&view].display_list.as_ref().expect("a display list");
    let faded = list
        .commands
        .iter()
        .any(|c| matches!(c, DisplayCommand::PushOpacity(o) if *o < 1.0));
    let text = list
        .commands
        .iter()
        .any(|c| matches!(c, DisplayCommand::Text { text, .. } if text.contains("Placeholder")));
    text && !faded
}

const STYLE: &str = "<style>body{margin:0;background:#fff}#n{height:68px;background:#eee}\
#h{height:300px;background:#123;color:#fff;font:40px sans-serif}#b{height:20px;background:#0a0}</style>\
<div id=n></div><div id=h style=\"opacity:0\">Placeholder hero</div><div id=b></div>";

/// Reduce's r4 page: the hero fades in over 200 ms and its script then
/// clears the inline `opacity:0`. Chrome paints it at opacity 1.
#[test]
fn r4_waapi_finished_clears_the_hero_and_paints_it() {
    let page = format!(
        "<!doctype html>{STYLE}<script>var h=document.getElementById('h');\
         try{{h.animate([{{opacity:0}},{{opacity:1}}],200).finished.then(()=>{{h.style.opacity=''}})}}\
         catch(e){{window.__err=String(e)}}</script>"
    );
    let (mut engine, view, _server) = load(&page);
    assert_eq!(read(&mut engine, view, "window.__err"), "undefined", "animate() is callable");
    pump(&mut engine, view, 400, 16);
    assert_eq!(read(&mut engine, view, "document.getElementById('h').style.opacity"), "");
    assert_eq!(opacity(&mut engine, view), 1.0);
    assert!(hero_painted(&engine, view), "the hero paints at opacity 1");
}

fn started(fill: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let (mut engine, view, server) = load(&format!("<!doctype html>{STYLE}"));
    run(
        &mut engine,
        view,
        &format!(
            "var h=document.getElementById('h'); window.log=[];\
             window.a=h.animate([{{opacity:0}},{{opacity:1}}],{{duration:200,fill:'{fill}'}});\
             a.finished.then(function(x){{log.push('resolved:'+(x===a))}},function(e){{log.push('rejected:'+e.name)}});"
        ),
    );
    (engine, view, server)
}

/// `fill: forwards` holds the last keyframe after the animation ends; midway
/// the value is between the two.
#[test]
fn fill_forwards_holds_the_final_opacity() {
    let (mut engine, view, _server) = started("forwards");
    assert_eq!(read(&mut engine, view, "a.playState"), "running");
    pump(&mut engine, view, 96, 16);
    let mid = opacity(&mut engine, view);
    assert!(mid > 0.2 && mid < 0.8, "midway opacity {mid}");
    pump(&mut engine, view, 400, 16);
    assert_eq!(read(&mut engine, view, "a.playState"), "finished");
    assert_eq!(read(&mut engine, view, "log.join()"), "resolved:true");
    assert_eq!(opacity(&mut engine, view), 1.0);
    assert!(hero_painted(&engine, view), "held at opacity 1");
    pump(&mut engine, view, 1000, 100);
    assert_eq!(opacity(&mut engine, view), 1.0, "still held");
}

/// `fill: none` (the default) drops the effect once finished: the inline
/// `opacity:0` shows again.
#[test]
fn fill_none_reverts_when_finished() {
    let (mut engine, view, _server) = started("none");
    pump(&mut engine, view, 400, 16);
    assert_eq!(read(&mut engine, view, "a.playState"), "finished");
    assert_eq!(opacity(&mut engine, view), 0.0);
    assert_eq!(read(&mut engine, view, "h.style.opacity"), "0");
}

/// `cancel()` rejects `finished` with an AbortError and removes the effect,
/// `fill: forwards` or not.
#[test]
fn cancel_rejects_finished_and_reverts() {
    let (mut engine, view, _server) = started("forwards");
    run(&mut engine, view, "a.oncancel=function(e){log.push('cancel:'+e.type)};");
    pump(&mut engine, view, 96, 16);
    assert!(opacity(&mut engine, view) > 0.0);
    run(&mut engine, view, "a.cancel();");
    pump(&mut engine, view, 32, 16);
    assert_eq!(read(&mut engine, view, "a.playState"), "idle");
    assert_eq!(read(&mut engine, view, "log.join()"), "rejected:AbortError,cancel:cancel");
    assert_eq!(opacity(&mut engine, view), 0.0);
    assert_eq!(read(&mut engine, view, "h.style.opacity"), "0");
}

/// `onfinish` and `addEventListener('finish')` both fire, once.
#[test]
fn onfinish_and_finish_listeners_fire() {
    let (mut engine, view, _server) = started("none");
    run(
        &mut engine,
        view,
        "a.onfinish=function(e){log.push('on:'+e.type+':'+(this===a))};\
         a.addEventListener('finish',function(e){log.push('listener:'+(e.target===a))});",
    );
    pump(&mut engine, view, 400, 16);
    assert_eq!(read(&mut engine, view, "log.join()"), "resolved:true,on:finish:true,listener:true");
}

/// The other keyframe and option shapes sites pass: an object of arrays, a
/// transform, `delay`, `finish()`, `pause()`, `currentTime`, `ready`, and
/// `getAnimations()`.
#[test]
fn keyframe_shapes_transform_and_controls() {
    let (mut engine, view, _server) = load(&format!("<!doctype html>{STYLE}"));
    run(
        &mut engine,
        view,
        "var h=document.getElementById('h'); window.log=[];\
         window.a=h.animate({transform:['translateX(0px)','translateX(100px)']},{duration:1000,delay:100,easing:'linear'});\
         a.ready.then(function(){log.push('ready')});",
    );
    assert_eq!(read(&mut engine, view, "h.getAnimations().length+','+document.getAnimations().length"), "1,1");
    pump(&mut engine, view, 600, 16);
    let t = read(&mut engine, view, "getComputedStyle(h).transform");
    let tx: f64 = t
        .trim_start_matches("matrix(")
        .trim_end_matches(')')
        .split(',')
        .nth(4)
        .unwrap_or_else(|| panic!("transform {t:?}"))
        .trim()
        .parse()
        .unwrap();
    assert!((tx - 50.0).abs() <= 12.0, "translateX about 50 at 600 ms with a 100 ms delay: {t}");
    // Chrome's pause is pending until the next frame (its currentTime moves
    // until then), so the held time is read a frame later.
    run(&mut engine, view, "a.pause();");
    pump(&mut engine, view, 32, 16);
    run(&mut engine, view, "window.held=a.currentTime;");
    pump(&mut engine, view, 300, 100);
    assert_eq!(read(&mut engine, view, "a.playState+','+(a.currentTime===held)"), "paused,true");
    run(&mut engine, view, "a.finish();");
    assert_eq!(read(&mut engine, view, "a.playState+','+a.currentTime"), "finished,1100");
    pump(&mut engine, view, 32, 16);
    assert_eq!(read(&mut engine, view, "log.join()"), "ready");
    assert_eq!(read(&mut engine, view, "h.getAnimations().length"), "0");
    assert_eq!(read(&mut engine, view, "h.getAttribute('style')"), "opacity:0");
}
