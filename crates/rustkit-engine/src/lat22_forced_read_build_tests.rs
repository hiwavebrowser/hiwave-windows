//! LAT22: a component page (hundreds of shadow roots sharing a large sheet)
//! that runs a Web Animation and reads a rect every frame. Pete's hand test
//! of 2026-11-11 on such a page: 429 forced layouts in one visit, 418 of
//! them `animation:rect`, each a full build of about 0.5 s, 224 s in all.

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

/// `cards` hosts, each with a shadow root holding the same large sheet
/// (`rules` rules) and a dozen elements; `extra` is light-DOM markup.
pub(crate) fn component_page(cards: usize, rules: usize, extra: &str) -> String {
    let mut sheet = String::from(":host{display:block;padding:2px} .row{display:flex;gap:4px} .c{padding:1px}");
    for i in 0..rules {
        sheet.push_str(&format!(
            ".k{i} .v{i} > span.t{i}:not(.off){{color:rgb({},{},{});margin:{}px;border-radius:{}px}}",
            i % 255,
            (i * 7) % 255,
            (i * 13) % 255,
            i % 5,
            i % 9
        ));
    }
    let shadow = format!(
        "<style>{sheet}</style><div class=row><div class=c><span>name</span></div><div class=c><span>score</span></div>\
         <div class=c><slot></slot></div></div><div class=row><span>reply</span><span>share</span><span>more</span></div>"
    );
    let mut page = String::from(
        "<!doctype html><html><head><style>body{margin:0;font:12px sans-serif} #a{width:50px;height:50px;background:green}\
         #hid{display:none}</style></head><body>",
    );
    page.push_str(extra);
    for i in 0..cards {
        page.push_str(&format!("<div class=card id=h{i}><p>comment {i}</p></div>"));
    }
    page.push_str(&format!(
        "<script>var SH={shadow:?};var hs=document.querySelectorAll('.card');\
         for(var i=0;i<hs.length;i++){{hs[i].attachShadow({{mode:'open'}}).innerHTML=SH;}}</script></body></html>"
    ));
    page
}

fn load(page: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 1024, height: 768 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

/// The loops a page might run each frame; which of them build the tree.
const SHAPES: &[(&str, &str)] = &[
    (
        "transform, light element",
        "var a=document.getElementById('a');a.animate([{transform:'translateX(0px)'},{transform:'translateX(300px)'}],{duration:4000,iterations:Infinity});\
         (function f(){requestAnimationFrame(function(){window.__r=a.getBoundingClientRect().left;f();});})()",
    ),
    (
        "transform inside a shadow tree",
        "var s=document.getElementById('h3').shadowRoot.querySelector('.row');s.animate([{transform:'translateX(0px)'},{transform:'translateX(30px)'}],{duration:4000,iterations:Infinity});\
         (function f(){requestAnimationFrame(function(){window.__r=s.getBoundingClientRect().left;f();});})()",
    ),
    (
        "opacity on a host, rect of another",
        "var h=document.getElementById('h5');h.animate([{opacity:1},{opacity:0.3}],{duration:4000,iterations:Infinity});\
         var o=document.getElementById('h9');(function f(){requestAnimationFrame(function(){window.__r=o.getBoundingClientRect().top;f();});})()",
    ),
    (
        "animation of an unpainted element",
        "var d=document.getElementById('hid');d.animate([{opacity:0},{opacity:1}],{duration:4000,iterations:Infinity});\
         var a=document.getElementById('a');(function f(){requestAnimationFrame(function(){window.__r=a.getBoundingClientRect().left;f();});})()",
    ),
    (
        "a new animation every frame",
        "var a=document.getElementById('a');(function f(){requestAnimationFrame(function(){\
         a.animate([{opacity:1},{opacity:0.9}],{duration:200});window.__r=a.getBoundingClientRect().left;f();});})()",
    ),
    (
        "a control every frame",
        "var a=document.getElementById('a');var an=a.animate([{transform:'translateX(0px)'},{transform:'translateX(300px)'}],{duration:4000,iterations:Infinity});\
         (function f(){requestAnimationFrame(function(){an.pause();an.play();window.__r=a.getBoundingClientRect().left;f();});})()",
    ),
    (
        "transform on an unpainted element",
        "var d=document.getElementById('hid');d.animate([{transform:'translateX(0px)'},{transform:'translateX(30px)'}],{duration:4000,iterations:Infinity});\
         var a=document.getElementById('a');(function f(){requestAnimationFrame(function(){window.__r=a.getBoundingClientRect().left;f();});})()",
    ),
    (
        "a property the engine does not interpolate",
        "var a=document.getElementById('a');a.animate([{'--p':'0'},{'--p':'1'}],{duration:4000,iterations:Infinity});\
         (function f(){requestAnimationFrame(function(){window.__r=a.getBoundingClientRect().left;f();});})()",
    ),
    (
        "a finished fill-forwards transform",
        "var a=document.getElementById('a');a.animate([{transform:'translateX(0px)'},{transform:'translateX(30px)'}],{duration:20,fill:'forwards'});\
         (function f(){requestAnimationFrame(function(){window.__r=a.getBoundingClientRect().left;f();});})()",
    ),
    (
        "a transform loop, then a style write and read once",
        "var a=document.getElementById('a');a.animate([{transform:'translateX(0px)'},{transform:'translateX(0px)'}],{duration:4000,iterations:Infinity});\
         var n=0;(function f(){requestAnimationFrame(function(){if(n++==5){a.animate([{transform:'scale(1)'},{transform:'scale(1)'}],{duration:4000,iterations:Infinity});}\
         window.__r=a.getBoundingClientRect().left;f();});})()",
    ),
    (
        "scroll-driven: a transform written from a scroll position",
        "var a=document.getElementById('a');var an=a.animate([{transform:'translateY(0px)'},{transform:'translateY(100px)'}],{duration:1000});an.pause();\
         (function f(){requestAnimationFrame(function(){an.currentTime=(window.scrollY%1000);window.__r=a.getBoundingClientRect().top;f();});})()",
    ),
    (
        "unchanging animation after a forced build",
        "var a=document.getElementById('a');a.animate([{transform:'none'},{transform:'none'}],{duration:4000,iterations:Infinity});\
         (function f(){requestAnimationFrame(function(){window.__r=a.getBoundingClientRect().left;f();});})()",
    ),
];

/// LAT22 exploration (run explicitly: `--ignored --nocapture`): for each
/// loop shape, over 10 frames after warm-up: forced builds, reads answered
/// from a frame, relayouts, the reasons a read built, and the last build's
/// cost with the shadow sheets' share.
#[test]
#[ignore]
fn lat22_shapes() {
    for (name, script) in SHAPES {
        let (mut engine, view, _server) = load(&component_page(300, 300, "<div id=a></div><div id=hid>hidden</div>"));
        engine.execute_script(view, script).unwrap();
        for _ in 0..4 {
            rt().block_on(engine.pump_live(view, 16));
        }
        let s0 = engine.relayout_stats(view).expect("view").clone();
        engine.views[&view].lay.forced_build_reasons.borrow_mut().clear();
        let started = std::time::Instant::now();
        for _ in 0..10 {
            rt().block_on(engine.pump_live(view, 16));
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let s = engine.relayout_stats(view).expect("view").clone();
        let lay = &engine.views[&view].lay;
        eprintln!(
            "LAT22 {name:<42} 10 frames {ms:>8.1} ms: forced builds {} ({:.1} ms), framed reads {}, relayouts {}, \
             paint-only frames {}, last build {:.1} ms, shadow {:?}, refusals {:?}",
            s.forced_layouts - s0.forced_layouts,
            s.forced_ms - s0.forced_ms,
            s.forced_frames - s0.forced_frames,
            s.relayouts - s0.relayouts,
            s.animation_frames_painted - s0.animation_frames_painted,
            s.last_build_ms,
            lay.last_shadow_build.get(),
            lay.forced_build_reasons.borrow()
        );
    }
}

/// LAT22: what a build of the component page costs by the size of the
/// sheet its shadow trees share (run explicitly: `--ignored`, parity
/// profile): the median build stage of 10 script relayouts.
#[test]
#[ignore]
fn lat22_build_by_shadow_sheet_size() {
    for rules in [0usize, 300, 1500] {
        let (mut engine, view, _server) = load(&component_page(300, rules, "<div id=a></div><p id=t>text</p>"));
        let mut builds = Vec::new();
        for i in 0..10 {
            engine
                .execute_script(view, &format!("document.getElementById('t').firstChild.data = 'text {i}';"))
                .unwrap();
            builds.push(engine.relayout_stats(view).expect("view").last_build_ms);
        }
        builds.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!(
            "LAT22BUILD 300 hosts, {rules:>4} shared rules: build {:.1} ms (median of 10), shadow sheets {:?}",
            builds[5],
            engine.views[&view].lay.last_shadow_build.get()
        );
    }
}


fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn frames(engine: &mut Engine, view: EngineViewId, n: usize) {
    for _ in 0..n {
        rt().block_on(engine.pump_live(view, 16));
    }
}

/// The hand-test signature (full build per read, then a paint-only frame,
/// no relayout): one animation holds a paint property that moves and a
/// property outside the paint list that does not (a constant `width`
/// beside a `transform`), and script reads a rect every frame. Nothing a
/// layout places changed, so no read builds the tree: each is answered
/// from a frame over the painted boxes, as the settle frame is.
#[test]
fn a_rect_read_after_an_unchanging_non_paint_animation_write_builds_no_tree() {
    let (mut engine, view, _server) = load(&component_page(40, 20, "<div id=a></div>"));
    engine
        .execute_script(
            view,
            "var a=document.getElementById('a');a.animate([{transform:'translateX(0px)',width:'50px'},{transform:'translateX(300px)',width:'50px'}],{duration:4000,iterations:Infinity});\
             window.__n=0;(function f(){requestAnimationFrame(function(){window.__r=a.getBoundingClientRect().width;window.__n++;f();});})()",
        )
        .unwrap();
    frames(&mut engine, view, 4);
    let s0 = engine.relayout_stats(view).expect("view").clone();
    let n0: f64 = read(&mut engine, view, "window.__n").parse().unwrap();
    frames(&mut engine, view, 8);
    let s = engine.relayout_stats(view).expect("view").clone();
    let n: f64 = read(&mut engine, view, "window.__n").parse().unwrap();
    assert!(n - n0 >= 6.0, "the loop ran: {n0} to {n}");
    assert_eq!(read(&mut engine, view, "window.__r"), "50", "the read sees the box");
    assert_eq!(
        (s.forced_layouts - s0.forced_layouts, s.relayouts - s0.relayouts),
        (0, 0),
        "no read built the tree and no frame laid the page out ({} reads answered from a frame)",
        s.forced_frames - s0.forced_frames
    );
    assert!(s.forced_frames - s0.forced_frames >= 6, "the reads were answered from frames");
}

/// A width that really animates still lays the page out for the read: the
/// rect is this frame's, the one a clean read finds.
#[test]
fn a_rect_read_after_a_moving_width_animation_sees_the_new_width() {
    let (mut engine, view, _server) = load(&component_page(10, 5, "<div id=a></div>"));
    engine
        .execute_script(
            view,
            "var a=document.getElementById('a');var an=a.animate([{width:'50px'},{width:'250px'}],{duration:1000});an.pause();",
        )
        .unwrap();
    frames(&mut engine, view, 2);
    engine
        .execute_script(view, "an.currentTime=500;window.__r=a.getBoundingClientRect().width;")
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__r"), "150", "Chrome 148: 150");
}

/// A read whose answer depends on the animated value: a transform (paint)
/// animated together with the unchanging width (not paint). The rect is the
/// transformed one, not the painted frame's stale one. Chrome for Testing
/// 148.0.7778.216 (docs/diagnostics/2026-11-12/lat22_rect_probe.js): left
/// 150, width 50 at half of a 0 to 300px translateX.
#[test]
fn a_rect_read_answered_from_a_frame_sees_the_animated_transform() {
    let (mut engine, view, _server) = load(&component_page(10, 5, "<div id=a></div>"));
    engine
        .execute_script(
            view,
            "var a=document.getElementById('a');\
             var an=a.animate([{transform:'translateX(0px)',width:'50px'},{transform:'translateX(300px)',width:'50.000001px'}],{duration:1000});an.pause();",
        )
        .unwrap();
    frames(&mut engine, view, 2);
    let s0 = engine.relayout_stats(view).expect("view").clone();
    engine
        .execute_script(view, "an.currentTime=500;var r=a.getBoundingClientRect();window.__r=r.left+'/'+r.width;")
        .unwrap();
    let s = engine.relayout_stats(view).expect("view").clone();
    assert_eq!(read(&mut engine, view, "window.__r"), "150/50", "Chrome 148: 150/50");
    assert_eq!(s.forced_layouts - s0.forced_layouts, 0, "answered from a frame, no build");
}

/// A page whose shadow trees use the selector kinds the index files rules
/// under (tag, class, id, attribute, universal, `:host`, `::slotted`,
/// `:not`, descendant and child combinators, pseudo-elements), in several
/// trees with different sheets, one nested in another.
fn indexed_selectors_page() -> String {
    let card = "<style>:host{display:block;padding:3px} :host(.wide){padding:9px} .row{display:flex;gap:4px} \
                .row > span{padding:1px} span.t:not(.off){color:rgb(10,20,30)} #x{margin:2px} [data-k]{border:1px solid red} \
                *{box-sizing:border-box} ::slotted(p){margin:5px} .row span::before{content:'b'} div .deep{padding-left:7px} \
                .unused .never{color:red}</style>\
                <div class=row><span class=t>a</span><span class='t off'>b</span><span id=x data-k=1>c</span></div>\
                <div><div><b class=deep>deep</b></div></div><slot></slot>";
    let inner = "<style>:host{display:inline-block} i{padding:2px} .n{margin:1px}</style><i class=n>inner</i>";
    let mut page = String::from("<!doctype html><html><head><style>body{margin:0} .card{margin:4px} p{color:blue}</style></head><body>");
    for i in 0..6 {
        let wide = if i % 2 == 0 { " wide" } else { "" };
        page.push_str(&format!("<div class='card{wide}' id=h{i}><p>light {i}</p></div>"));
    }
    page.push_str(&format!(
        "<p id=t>text</p><script>var C={card:?};var I={inner:?};var hs=document.querySelectorAll('.card');\
         for(var i=0;i<hs.length;i++){{var r=hs[i].attachShadow({{mode:'open'}});r.innerHTML=C;\
         if(i%3==0){{var d=document.createElement('div');r.appendChild(d);d.attachShadow({{mode:'open'}}).innerHTML=I;}}}}</script></body></html>"
    ));
    page
}

fn built_dump(page: &str) -> (String, u32) {
    let (mut engine, view, _server) = load(page);
    engine
        .execute_script(view, "document.getElementById('t').firstChild.data = 'changed';")
        .unwrap();
    let lay = &engine.views[&view].lay;
    let unindexed = lay.last_unindexed_cascades.get();
    (format!("{:?}", engine.views[&view].layout.as_ref().expect("laid out")), unindexed)
}

/// LAT22 (3): every element of a shadow tree is matched through a rule
/// index of its tree's sheets, as the document's elements are: no cascade
/// of the build visits every rule.
#[test]
fn shadow_tree_elements_cascade_through_a_rule_index() {
    let (_, unindexed) = built_dump(&component_page(40, 50, "<div id=a></div><p id=t>text</p>"));
    assert_eq!(unindexed, 0, "cascades that visited every rule of their sheets");
}

/// The index changes no style: the laid-out tree of a page of shadow
/// trees, with and without it, is the same box for box.
#[test]
fn the_shadow_rule_index_builds_the_same_boxes() {
    for page in [indexed_selectors_page(), component_page(12, 30, "<div id=a></div><p id=t>text</p>")] {
        set_tree_rule_index_off_for_test(true);
        let (without, unindexed) = built_dump(&page);
        set_tree_rule_index_off_for_test(false);
        assert!(unindexed > 0, "the switch turns the index off");
        let (with, _) = built_dump(&page);
        assert!(without.len() > 2000, "a real tree: {}", without.len());
        assert!(with == without, "the indexed build differs from the unindexed one");
    }
}

/// Many roots whose sheets differ but have one shape (the same selectors,
/// other values) and the same ids; each holds a nested tree of the same
/// kind. Odd roots' `:host` rules size their hosts; the even roots' hosts
/// share one style, which is where sharing went wrong (a host with `:host`
/// rules is never shared, nor its children). Every element takes its own tree's style, with or without the
/// rule index and the view cache. Until 2026-11-12 style sharing keyed rule
/// numbers without saying whose index numbered them, so every `#x` after
/// the first took the first's style (`5px` everywhere).
const SAME_SHAPE_PAGE: &str = "<!doctype html><html><head><style>body{margin:0}</style></head><body>\
    <div id=hosts></div><p id=t>text</p><script>\
    function css(a,b){return '<style id=st>#x{padding-left:'+a+'px}'+(b?':host{padding-top:'+b+'px}':'')+'</style>';}\
    for(var i=0;i<8;i++){var h=document.createElement('div');h.className='h';\
    document.getElementById('hosts').appendChild(h);\
    var r=h.attachShadow({mode:'open'});r.innerHTML=css(1+i%3,i%2*2)+'<div id=x>x</div><div id=n></div>';\
    r.querySelector('#n').attachShadow({mode:'open'}).innerHTML=css(4+i%3,i%2)+'<div id=x>x</div>';}\
    function each(f){return Array.prototype.map.call(document.querySelectorAll('.h'),f).join('/');}\
    </script></body></html>";

fn same_shape_expected() -> String {
    (0..8)
        .map(|i| format!("{}px {}px {}px {}px", 1 + i % 3, i % 2 * 2, 4 + i % 3, i % 2))
        .collect::<Vec<_>>()
        .join("/")
}

const SAME_SHAPE_READ: &str = "each(function(h){var r=h.shadowRoot,n=r.querySelector('#n');\
    return [getComputedStyle(r.querySelector('#x')).paddingLeft,getComputedStyle(h).paddingTop,\
    getComputedStyle(n.shadowRoot.querySelector('#x')).paddingLeft,getComputedStyle(n).paddingTop].join(' ');})";

#[test]
fn roots_with_differing_sheets_of_one_shape_match_their_own() {
    let want = same_shape_expected();
    for cache_off in [true, false] {
        crate::shadow_tree::set_view_cache_off_for_test(cache_off);
        set_tree_rule_index_off_for_test(true);
        let (without, _) = built_dump(SAME_SHAPE_PAGE);
        set_tree_rule_index_off_for_test(false);
        let (with, _) = built_dump(SAME_SHAPE_PAGE);
        let (mut engine, view, _server) = load(SAME_SHAPE_PAGE);
        let got = read(&mut engine, view, SAME_SHAPE_READ);
        crate::shadow_tree::set_view_cache_off_for_test(false);
        assert_eq!(got, want, "cache off: {cache_off}");
        assert!(with == without, "cache off: {cache_off}; the indexed build differs from the unindexed one");
    }
}

/// LAT22: the view keeps its shadow tree styles across builds. A build
/// after a script write elsewhere reads no shadow sheet's text and parses
/// none.
#[test]
fn a_later_build_reads_no_shadow_sheet_text() {
    let (mut engine, view, _server) = load(&component_page(40, 50, "<div id=a></div><p id=t>text</p>"));
    for i in 0..2 {
        engine
            .execute_script(view, &format!("document.getElementById('t').firstChild.data = 'changed {i}';"))
            .unwrap();
    }
    let shadow = engine.views[&view].lay.last_shadow_build.get();
    assert_eq!(shadow.roots, 40, "{shadow:?}");
    assert_eq!((shadow.texts_read, shadow.parsed), (0, 0), "{shadow:?}");
}

/// Two hosts with shadow roots; `#x` in each is what its sheets style.
const CACHE_PAGE: &str = "<!doctype html><html><head><style>body{margin:0}</style></head><body>\
    <div id=h1></div><div id=h2></div><p id=t>text</p><script>\
    var S='<style id=st>#x{padding-left:3px}</style><div id=x>x</div>';\
    var r1=document.getElementById('h1').attachShadow({mode:'open'});r1.innerHTML=S;\
    var r2=document.getElementById('h2').attachShadow({mode:'open'});r2.innerHTML=S;\
    function pad(r){return getComputedStyle(r.querySelector('#x')).paddingLeft;}\
    </script></body></html>";

/// Every way a shadow root's sheets change is seen by the next build, with
/// the cache as without it: the style's text, a second `<style>`, a removed
/// one, its `media` attribute, the root's `adoptedStyleSheets` (set, then
/// replaced), and a resize across an `@media` threshold. The other root,
/// whose sheets did not change, keeps its style throughout.
#[test]
fn each_change_to_a_shadow_roots_sheets_restyles_it() {
    let steps: &[(&str, &str)] = &[
        ("", "3px/3px"),
        ("r1.querySelector('#st').textContent='#x{padding-left:5px}';", "5px/3px"),
        ("var n=document.createElement('style');n.id='s2';n.textContent='#x{padding-left:7px}';r1.appendChild(n);", "7px/3px"),
        ("r1.querySelector('#s2').remove();", "5px/3px"),
        ("r1.querySelector('#st').setAttribute('media','print');", "0px/3px"),
        ("r1.querySelector('#st').removeAttribute('media');", "5px/3px"),
        ("var a=new CSSStyleSheet();a.replaceSync('#x{padding-left:9px}');r1.adoptedStyleSheets=[a];", "9px/3px"),
        ("var b=new CSSStyleSheet();b.replaceSync('#x{padding-left:11px}');r1.adoptedStyleSheets=[b];", "11px/3px"),
        ("r1.adoptedStyleSheets=[];", "5px/3px"),
        ("r2.querySelector('#st').textContent='@media (max-width:500px){#x{padding-left:13px}}';", "5px/0px"),
    ];
    for cache_off in [true, false] {
        crate::shadow_tree::set_view_cache_off_for_test(cache_off);
        let (mut engine, view, _server) = load(CACHE_PAGE);
        for (script, want) in steps {
            // A build in between, so the cache holds the state before.
            engine.execute_script(view, "document.getElementById('t').firstChild.data += '.';").unwrap();
            engine.execute_script(view, script).unwrap();
            assert_eq!(read(&mut engine, view, "pad(r1)+'/'+pad(r2)"), *want, "cache off: {cache_off}; step: {script}");
        }
        engine.resize_view(view, Bounds { x: 0, y: 0, width: 400, height: 768 }).expect("resize");
        assert_eq!(read(&mut engine, view, "pad(r1)+'/'+pad(r2)"), "5px/13px", "cache off: {cache_off}; after the resize");
        engine.resize_view(view, Bounds { x: 0, y: 0, width: 1024, height: 768 }).expect("resize");
        assert_eq!(read(&mut engine, view, "pad(r1)+'/'+pad(r2)"), "5px/0px", "cache off: {cache_off}; resized back");
    }
    crate::shadow_tree::set_view_cache_off_for_test(false);
}

/// The cache changes no box: the laid-out tree after a run of builds, with
/// and without it.
#[test]
fn the_view_tree_style_cache_builds_the_same_boxes() {
    let page = indexed_selectors_page();
    let dump = |cache_off: bool| {
        crate::shadow_tree::set_view_cache_off_for_test(cache_off);
        let (mut engine, view, _server) = load(&page);
        for script in [
            "document.getElementById('t').firstChild.data = 'one';",
            "document.getElementById('h1').classList.add('wide');",
            "document.getElementById('h2').shadowRoot.querySelector('style').textContent += '.row{gap:9px}';",
            "document.getElementById('t').firstChild.data = 'two';",
        ] {
            engine.execute_script(view, script).unwrap();
        }
        let out = format!("{:?}", engine.views[&view].layout.as_ref().expect("laid out"));
        crate::shadow_tree::set_view_cache_off_for_test(false);
        out
    };
    let (without, with) = (dump(true), dump(false));
    assert!(without.len() > 2000);
    assert!(with == without, "the cached build differs from the uncached one");
}

