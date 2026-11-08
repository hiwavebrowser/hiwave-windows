//! The rest of Web Animations (S13c): animated values live in the engine's
//! animated-style layer (the `style` attribute stays untouched, as in
//! Chrome; #881 wrote them through the inline style), `getAnimations()`
//! lists the CSS-created `CSSAnimation`/`CSSTransition` objects too,
//! `playbackRate`/`reverse()`/`updatePlaybackRate()`, composite
//! `add`/`accumulate`, and `new Animation(new KeyframeEffect(...),
//! document.timeline)`.
//!
//! The oracle is Chrome 143 through `tools/parity_oracle/waapi_layer_log.mjs`.

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn load(page: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine
        .execute_script(view, &format!("String({expr})"))
        .unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn run(engine: &mut Engine, view: EngineViewId, script: &str) {
    engine.execute_script(view, script).unwrap();
}

fn pump(engine: &mut Engine, view: EngineViewId, total: u64, step: u64) {
    let rt = rt();
    let mut t = 0;
    while t < total {
        rt.block_on(engine.pump_live(view, step));
        t += step;
    }
}

fn computed(engine: &mut Engine, view: EngineViewId, prop: &str) -> String {
    read(
        engine,
        view,
        &format!("getComputedStyle(document.getElementById('h')).getPropertyValue('{prop}')"),
    )
}

fn opacity(engine: &mut Engine, view: EngineViewId) -> f64 {
    let s = computed(engine, view, "opacity");
    s.parse()
        .unwrap_or_else(|_| panic!("computed opacity {s:?}"))
}

fn num(engine: &mut Engine, view: EngineViewId, expr: &str) -> f64 {
    let s = read(engine, view, expr);
    s.parse().unwrap_or_else(|_| panic!("{expr} = {s:?}"))
}

fn page(css: &str, body: &str, script: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0}}#h{{width:100px;height:100px;background:#123}}{css}</style>\
         {body}<script>var h=document.getElementById('h');window.log=[];{script}</script>"
    )
}

/// A MutationObserver on `style` sees no mutation during an animation and
/// the attribute reads as the page wrote it; `commitStyles()` is the one
/// write to it.
#[test]
fn an_animation_leaves_the_style_attribute_and_observers_alone() {
    let (mut engine, view, _server) = load(&page(
        "",
        "<div id=h style=\"opacity:0\"></div>",
        "window.recs=0;new MutationObserver(function(m){recs+=m.length}).observe(h,{attributes:true});",
    ));
    // Started after load: the load phase runs an animation-frame chain to
    // its timer horizon, so one started by an inline script is over by
    // the time the page shows (a gap of the load, not of this layer).
    run(
        &mut engine,
        view,
        "window.a=h.animate([{opacity:0},{opacity:1}],{duration:200,fill:'forwards',easing:'linear'});",
    );
    pump(&mut engine, view, 96, 16);
    let mid = opacity(&mut engine, view);
    assert!(mid > 0.2 && mid < 0.8, "midway opacity {mid}");
    assert_eq!(
        read(&mut engine, view, "h.getAttribute('style')"),
        "opacity:0"
    );
    assert_eq!(read(&mut engine, view, "recs"), "0");
    pump(&mut engine, view, 300, 16);
    assert_eq!(opacity(&mut engine, view), 1.0);
    assert_eq!(
        read(&mut engine, view, "h.getAttribute('style')"),
        "opacity:0"
    );
    assert_eq!(read(&mut engine, view, "recs"), "0");
    run(&mut engine, view, "a.commitStyles();");
    pump(&mut engine, view, 16, 16);
    assert_eq!(read(&mut engine, view, "h.style.opacity"), "1");
    assert_eq!(read(&mut engine, view, "recs"), "1");
}

/// `getAnimations()` lists the CSS-created animations and transitions as
/// `CSSAnimation`/`CSSTransition` (transitions first), with a live
/// `currentTime`, and `pause()`/`play()`/`cancel()` act on them.
#[test]
fn get_animations_lists_css_animations_and_transitions() {
    let (mut engine, view, _server) = load(&page(
        "#h{opacity:0;animation:fade 1s linear forwards;transition:width 1s linear}#h.w{width:200px}\
         @keyframes fade{to{opacity:1}}",
        "<div id=h></div>",
        "h.addEventListener('animationcancel',function(e){log.push('cancel:'+e.animationName)});",
    ));
    pump(&mut engine, view, 16, 16);
    assert_eq!(
        read(
            &mut engine,
            view,
            "h.getAnimations().length+','+document.getAnimations().length"
        ),
        "1,1"
    );
    run(&mut engine, view, "window.a=h.getAnimations()[0];");
    assert_eq!(
        read(
            &mut engine,
            view,
            "[a instanceof CSSAnimation, a instanceof Animation, a.animationName, a.playState, typeof a.currentTime, \
              a.effect.target===h, a.effect.getTiming().duration, a.effect.getTiming().fill, a.timeline===document.timeline].join()"
        ),
        "true,true,fade,running,number,true,1000,forwards,true"
    );
    let t1 = num(&mut engine, view, "a.currentTime");
    pump(&mut engine, view, 100, 10);
    let t2 = num(&mut engine, view, "a.currentTime");
    assert!(
        (t2 - t1 - 100.0).abs() <= 20.0,
        "currentTime moved {t1} -> {t2}"
    );
    assert_eq!(
        read(&mut engine, view, "h.getAnimations()[0]===a"),
        "true",
        "the same object again"
    );
    run(&mut engine, view, "h.classList.add('w');");
    pump(&mut engine, view, 16, 16);
    assert_eq!(
        read(
            &mut engine,
            view,
            "h.getAnimations().map(function(x){return x.constructor.name+':'+(x.transitionProperty||x.animationName)}).join()"
        ),
        "CSSTransition:width,CSSAnimation:fade"
    );
    run(&mut engine, view, "a.pause();");
    pump(&mut engine, view, 16, 16);
    let held = opacity(&mut engine, view);
    assert_eq!(read(&mut engine, view, "a.playState"), "paused");
    pump(&mut engine, view, 200, 20);
    assert_eq!(opacity(&mut engine, view), held, "paused holds");
    run(&mut engine, view, "a.play();");
    pump(&mut engine, view, 100, 10);
    assert!(opacity(&mut engine, view) > held, "running again");
    run(&mut engine, view, "a.cancel();");
    pump(&mut engine, view, 32, 16);
    assert_eq!(
        opacity(&mut engine, view),
        0.0,
        "cancelled: the box's own opacity"
    );
    assert_eq!(read(&mut engine, view, "log.join()"), "cancel:fade");
    assert_eq!(
        read(
            &mut engine,
            view,
            "h.getAnimations().filter(function(x){return x instanceof CSSAnimation}).length"
        ),
        "0"
    );
}

/// `playbackRate` scales the clock, `reverse()` runs it back, and
/// `updatePlaybackRate()` takes a new rate.
#[test]
fn playback_rate_and_reverse_run_the_clock_both_ways() {
    let (mut engine, view, _server) = load(&page(
        "",
        "<div id=h></div>",
        "window.a=h.animate([{opacity:0},{opacity:1}],{duration:200,fill:'both',easing:'linear'});a.playbackRate=2;",
    ));
    pump(&mut engine, view, 128, 16);
    assert_eq!(
        read(&mut engine, view, "a.playState+','+a.currentTime"),
        "finished,200"
    );
    assert_eq!(opacity(&mut engine, view), 1.0);
    run(&mut engine, view, "a.reverse();");
    pump(&mut engine, view, 16, 16);
    assert_eq!(
        read(&mut engine, view, "a.playState+','+a.playbackRate"),
        "running,-2"
    );
    pump(&mut engine, view, 32, 16);
    let back = opacity(&mut engine, view);
    assert!(back < 1.0 && back > 0.0, "running back: {back}");
    pump(&mut engine, view, 100, 16);
    // Chrome reads a currentTime at or past the start here (-99.996 on
    // the oracle: the hold is not clamped at 0 when nothing seeks).
    assert_eq!(
        read(&mut engine, view, "a.playState+','+(a.currentTime<=0)"),
        "finished,true"
    );
    assert_eq!(opacity(&mut engine, view), 0.0);
    run(&mut engine, view, "a.updatePlaybackRate(0.5);a.play();");
    pump(&mut engine, view, 16, 16);
    assert_eq!(
        read(&mut engine, view, "a.playbackRate+','+a.playState"),
        "0.5,running"
    );
    pump(&mut engine, view, 100, 10);
    let slow = opacity(&mut engine, view);
    assert!(
        (slow - 0.3).abs() <= 0.12,
        "about 60 ms in at half speed: {slow}"
    );
}

/// Composite `add` and `accumulate` build on the underlying value: a
/// translate added to a translate, opacity summed, scale accumulated.
#[test]
fn composite_add_and_accumulate_build_on_the_underlying_value() {
    let (mut engine, view, _server) = load(&page(
        "#h{opacity:.5;transform:translateX(100px)}",
        "<div id=h></div>",
        "window.a=h.animate({transform:['translateX(50px)','translateX(50px)'],opacity:[0.25,0.25]},\
         {duration:100,composite:'add',fill:'forwards'});",
    ));
    pump(&mut engine, view, 150, 10);
    assert_eq!(
        computed(&mut engine, view, "transform"),
        "matrix(1, 0, 0, 1, 150, 0)"
    );
    assert_eq!(opacity(&mut engine, view), 0.75);
    run(
        &mut engine,
        view,
        "a.cancel();h.style.transform='scale(2)';\
         window.b=h.animate({transform:['scale(2)','scale(2)']},{duration:100,composite:'accumulate',fill:'forwards'});",
    );
    pump(&mut engine, view, 150, 10);
    assert_eq!(
        computed(&mut engine, view, "transform"),
        "matrix(3, 0, 0, 3, 0, 0)"
    );
    assert_eq!(opacity(&mut engine, view), 0.5, "the cancelled add is gone");
}

/// `new Animation(new KeyframeEffect(...), document.timeline)`: idle until
/// played, then driven by the document timeline.
#[test]
fn animation_constructor_and_document_timeline() {
    let (mut engine, view, _server) = load(&page(
        "#h{opacity:0}",
        "<div id=h></div>",
        "window.e=new KeyframeEffect(h,[{opacity:0},{opacity:1}],{duration:100,fill:'forwards',easing:'linear'});\
         window.a=new Animation(e,document.timeline);",
    ));
    assert_eq!(
        read(&mut engine, view, "[a.playState,a.effect===e,a.timeline===document.timeline,typeof document.timeline.currentTime,h.getAnimations().length].join()"),
        "idle,true,true,number,0"
    );
    let t0 = num(&mut engine, view, "document.timeline.currentTime");
    run(&mut engine, view, "a.play();");
    pump(&mut engine, view, 50, 10);
    // A played animation starts on the next frame: Chrome reads 0.25 at
    // 50 ms, not 0.5.
    let mid = opacity(&mut engine, view);
    assert!(mid > 0.15 && mid < 0.6, "midway: {mid}");
    let progress = num(&mut engine, view, "e.getComputedTiming().progress");
    assert!(
        (progress - mid).abs() <= 0.02,
        "progress {progress} is the opacity {mid}"
    );
    let t1 = num(&mut engine, view, "document.timeline.currentTime");
    assert!(t1 >= t0 + 40.0, "the timeline moved {t0} -> {t1}");
    pump(&mut engine, view, 100, 10);
    assert_eq!(read(&mut engine, view, "a.playState"), "finished");
    assert_eq!(opacity(&mut engine, view), 1.0);
}

/// (a) A `CSSAnimation` object is one engine animation, not one
/// `animation-name` slot: a class swap that replaces the name mid-run gives
/// a new object with the new name, the old one's `finished` rejected and
/// its state `idle`, the new one's pending. An animation that ended and is
/// then replaced keeps its resolved `finished`, and the replacement is a
/// third object (Chrome: a new `CSSAnimation` per animation).
#[test]
fn a_swapped_animation_name_is_a_new_css_animation_object() {
    let (mut engine, view, _server) = load(&page(
        "#h{opacity:0}#h.enter{animation:fadeIn 1s linear forwards}#h.leave{animation:fadeOut 1s linear forwards}\
         @keyframes fadeIn{to{opacity:1}}@keyframes fadeOut{from{opacity:1}to{opacity:0}}",
        "<div id=h class=enter></div>",
        "window.watch=function(tag,a){a.finished.then(function(){log.push(tag+':res')},function(e){log.push(tag+':rej:'+e.name)});\
         a.oncancel=function(){log.push(tag+':cancel')}};",
    ));
    pump(&mut engine, view, 16, 16);
    run(&mut engine, view, "window.a=h.getAnimations()[0];watch('a',a);");
    assert_eq!(read(&mut engine, view, "a.animationName+','+a.playState"), "fadeIn,running");
    run(&mut engine, view, "h.classList.replace('enter','leave');");
    pump(&mut engine, view, 32, 16);
    run(&mut engine, view, "window.b=h.getAnimations()[0];watch('b',b);");
    pump(&mut engine, view, 16, 16);
    assert_eq!(
        read(
            &mut engine,
            view,
            "[b!==a, a.animationName, b.animationName, a.playState, b.playState, a.currentTime===null].join()"
        ),
        "true,fadeIn,fadeOut,idle,running,true"
    );
    assert_eq!(read(&mut engine, view, "log.join()"), "a:rej:AbortError,a:cancel", "the old one was cancelled, the new one is pending");
    pump(&mut engine, view, 1100, 50);
    assert_eq!(read(&mut engine, view, "b.playState+','+log.join()"), "finished,a:rej:AbortError,a:cancel,b:res");
    // The ended one is replaced: its `finished` stays resolved, it goes
    // idle, and the replacement is a new object with a pending `finished`.
    run(&mut engine, view, "h.classList.replace('leave','enter');");
    pump(&mut engine, view, 32, 16);
    run(&mut engine, view, "window.c=h.getAnimations()[0];watch('c',c);");
    pump(&mut engine, view, 16, 16);
    assert_eq!(
        read(
            &mut engine,
            view,
            "[c!==a, c!==b, c.animationName, c.playState, b.playState, h.getAnimations().length].join()"
        ),
        "true,true,fadeIn,running,idle,1"
    );
    assert_eq!(read(&mut engine, view, "log.join()"), "a:rej:AbortError,a:cancel,b:res");
}

/// (b) `display: none` then shown again restarts the CSS animation (#885)
/// as a new `CSSAnimation` object, whether or not a frame ran in between;
/// the hidden one's wrapper is cancelled.
#[test]
fn a_hidden_then_shown_css_animation_is_a_new_object() {
    let (mut engine, view, _server) = load(&page(
        "#h{opacity:0;animation:fade 1s linear forwards}#h.hide{display:none}@keyframes fade{to{opacity:1}}",
        "<div id=h></div>",
        "window.watch=function(tag,a){a.finished.then(function(){log.push(tag+':res')},function(e){log.push(tag+':rej:'+e.name)});\
         a.oncancel=function(){log.push(tag+':cancel')}};",
    ));
    pump(&mut engine, view, 16, 16);
    run(&mut engine, view, "window.a=h.getAnimations()[0];watch('a',a);");
    run(&mut engine, view, "h.classList.add('hide');");
    pump(&mut engine, view, 32, 16);
    run(&mut engine, view, "h.classList.remove('hide');");
    pump(&mut engine, view, 32, 16);
    run(&mut engine, view, "window.b=h.getAnimations()[0];watch('b',b);");
    pump(&mut engine, view, 16, 16);
    assert_eq!(
        read(&mut engine, view, "[b!==a, a.playState, b.playState, b.currentTime<200, log.join()].join()"),
        "true,idle,running,true,a:rej:AbortError,a:cancel"
    );
    // Hidden and shown within one task, with a forced style read between:
    // still a new animation and a new object.
    run(&mut engine, view, "h.classList.add('hide');h.offsetWidth;h.classList.remove('hide');");
    pump(&mut engine, view, 32, 16);
    run(&mut engine, view, "window.c=h.getAnimations()[0];watch('c',c);");
    pump(&mut engine, view, 16, 16);
    assert_eq!(
        read(&mut engine, view, "[c!==b, c!==a, b.playState, c.playState, c.currentTime<200, log.join()].join()"),
        "true,true,idle,running,true,a:rej:AbortError,a:cancel,b:rej:AbortError,b:cancel"
    );
}

/// (c) The CSS dropping the name mid-run cancels the wrapper: `finished`
/// rejects with `AbortError`, a `cancel` event fires, and it reads as
/// `idle` with a null `currentTime`.
#[test]
fn dropping_the_animation_name_mid_run_cancels_its_wrapper() {
    let (mut engine, view, _server) = load(&page(
        "#h{opacity:0}#h.go{animation:fade 1s linear forwards}@keyframes fade{to{opacity:1}}",
        "<div id=h class=go></div>",
        "",
    ));
    pump(&mut engine, view, 16, 16);
    run(
        &mut engine,
        view,
        "window.a=h.getAnimations()[0];\
         a.finished.then(function(){log.push('res')},function(e){log.push('rej:'+e.name)});\
         a.addEventListener('cancel',function(e){log.push('cancel:'+e.type)});",
    );
    assert_eq!(read(&mut engine, view, "a.playState"), "running");
    run(&mut engine, view, "h.classList.remove('go');");
    pump(&mut engine, view, 32, 16);
    assert_eq!(
        read(&mut engine, view, "[a.playState, a.currentTime, h.getAnimations().length, log.join()].join()"),
        "idle,,0,rej:AbortError,cancel:cancel"
    );
}

/// `composite: 'add'` on `opacity` clamps at 1, as the property's range
/// does (`.75 + .5` is 1, not 1.25).
#[test]
fn an_opacity_add_clamps_at_one() {
    let (mut engine, view, _server) = load(&page(
        "#h{opacity:.75}",
        "<div id=h></div>",
        "window.a=h.animate({opacity:[0.5,0.5]},{duration:100,composite:'add',fill:'forwards'});",
    ));
    pump(&mut engine, view, 150, 10);
    assert_eq!(opacity(&mut engine, view), 1.0);
}
