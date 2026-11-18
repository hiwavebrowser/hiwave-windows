//! Pins from the independent review of the Speedometer stack (R1 MED 1, 2
//! and 5).

use super::*;

fn page(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, html).expect("load");
    (engine, view)
}

fn js(engine: &mut Engine, view: EngineViewId, script: &str) -> String {
    let value = engine.execute_script(view, script).expect("script");
    value.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&value).to_string()
}

/// MED 5: the engine's own entry points are not the page's to call or to
/// replace, and nothing the page does makes an event trusted.
#[test]
fn a_page_cannot_reach_the_engines_event_internals_or_forge_trust() {
    let (mut engine, view) = page(
        "<!doctype html><body style=\"margin:0\"><input id=f style=\"display:block\">\
         <svg id=s width=200 height=100 style=\"display:block\"><rect id=r width=200 height=100></rect></svg>\
         <script>var seen = [];\
         document.addEventListener('keydown', function (e) { seen.push('keydown ' + e.isTrusted); }, true);\
         document.addEventListener('textInput', function (e) { seen.push('textInput ' + e.isTrusted + ' ' + e.data); }, true);\
         document.addEventListener('mousemove', function (e) { seen.push('mousemove ' + e.target.id + ' ' + e.isTrusted); }, true);\
         document.addEventListener('x', function (e) { seen.push('x ' + e.isTrusted); }, true);</script>",
    );
    // None of the stack's internals is a property the page can read.
    assert_eq!(
        js(
            &mut engine,
            view,
            "[typeof document.__rkFireTextInput, typeof document.__rkFireKey, typeof window.__rkNewTextEvent, \
              typeof window.__rkSvgHit, typeof document.__rkViewFocused].join(' ')"
        ),
        "undefined undefined undefined undefined undefined"
    );
    // Defining them changes nothing the engine does.
    js(
        &mut engine,
        view,
        "window.__rkSvgHit = function () { return document.body; }; \
         document.__rkFireTextInput = function () { seen.push('page ran the text hook'); return false; }; \
         document.__rkFireKey = function () { seen.push('page ran the key hook'); return false; }; \
         window.__rkNewTextEvent = function () { seen.push('page made the text event'); }; \
         document.__rkViewFocused = false; 0",
    );
    assert_eq!(js(&mut engine, view, "String(document.hasFocus())"), "true", "the page cannot unfocus itself");
    engine.mouse_move_at_point(view, 50.0, 60.0);
    assert_eq!(engine.focus_at_point(view, 20.0, 8.0).as_deref(), Some("input"));
    engine.handle_text_key(view, 0, "q", false, false, false);
    // What script dispatches is never trusted, whatever it writes first.
    js(
        &mut engine,
        view,
        "var e = new Event('x'); try { e.isTrusted = true; } catch (_) {} document.dispatchEvent(e); \
         var k = new KeyboardEvent('keydown', { key: 'z' }); try { k.isTrusted = true; } catch (_) {} document.dispatchEvent(k); \
         try { Object.defineProperty(k, 'isTrusted', { value: true }); } catch (_) {} seen.push('redefined ' + k.isTrusted); \
         try { document.dispatchEvent({ type: 'x', isTrusted: true, bubbles: false, cancelable: false, target: null }); } \
         catch (e) { seen.push('look-alike ' + e.name); } 0",
    );
    assert_eq!(
        js(&mut engine, view, "seen.join(' | ')"),
        // (A look-alike event is not dispatched at all: a TypeError, as in
        // Chrome 148; pinned in dispatch_event_argument_tests.)
        "mousemove r true | keydown true | textInput true q | x false | keydown false | redefined false | look-alike TypeError"
    );
}

/// MED 3, as a page meets it: a box in a narrow parent is moved out of the
/// parent by a transform Web Animation, and a script reads
/// `elementFromPoint` in the frame it moves (answered from the animation
/// frame placed on a copy of the painted boxes). The pointer was over the
/// page before, so the painted boxes carry the hit test's marks from when
/// the box was inside its parent; the copy must not keep them.
#[test]
fn a_box_a_web_animation_moves_out_of_its_parent_is_hit_where_the_frame_puts_it() {
    let server = crate::script_net_tests::serve_routes(vec![(
        "/",
        "text/html",
        "<!doctype html><style>body{margin:0}#p{width:40px;height:40px}#a{width:40px;height:40px;background:green}\
         #tall{height:400px}</style><div id=p><div id=a></div></div><div id=tall></div>\
         <script>var a = document.getElementById('a'); window.seen = [];</script>"
            .to_string(),
    )]);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    // In place for the first half, then 300px to the right, in one step.
    engine
        .execute_script(
            view,
            "a.animate([{transform:'translateX(0px)',easing:'steps(1, end)'},{transform:'translateX(0px)',offset:0.5,easing:'steps(1, start)'},\
             {transform:'translateX(300px)'}],{duration:600,fill:'forwards'});\
             (function f(){requestAnimationFrame(function(){\
             var hit = document.elementFromPoint(320, 20);\
             seen.push(Math.round(a.getBoundingClientRect().left) + ':' + (hit ? hit.id || hit.localName : 'null'));f();});})()",
        )
        .unwrap();
    for _ in 0..70 {
        // The pointer is over the page: the painted boxes are marked.
        engine.mouse_move_at_point(view, 20.0, 20.0);
        rt.block_on(engine.pump_live(view, 16));
    }
    let seen = js(&mut engine, view, "seen.join(' ')");
    let reads: Vec<&str> = seen.split(' ').collect();
    assert!(reads.iter().any(|r| r.starts_with("0:")), "read while in place: {seen}");
    assert!(reads.iter().any(|r| r.starts_with("300:")), "read after the move: {seen}");
    for read in &reads {
        let (left, hit) = read.split_once(':').unwrap();
        if left == "300" {
            assert_eq!(hit, "a", "the box is hit where the frame put it: {seen}");
        }
    }
}

/// An element that had more than 32 attributes, lost some and gained one
/// (script's, or the inline-SVG serializer's own when an author paint rule
/// applies: it takes the presentation attributes the rule beats away and
/// adds `style`). The attribute store's name set went stale there: the new
/// attribute was not found, and the SVG serializer, which indexed each name
/// it listed, panicked the page thread on a live page.
#[test]
fn an_svg_shape_with_many_attributes_and_an_author_paint_rule_serializes() {
    let mut attrs = String::new();
    for i in 0..33 {
        attrs.push_str(&format!(" data-a{i}=\"{i}\""));
    }
    let (mut engine, view) = page(&format!(
        "<!doctype html><style>#r {{ fill: green; stroke: blue }}</style><body style=\"margin:0\">\
         <svg id=s width=200 height=100 style=\"display:block\">\
         <rect id=r width=200 height=100 fill=red stroke=red{attrs}></rect></svg>\
         <div id=d></div>"
    ));
    // Script's own shrink-then-add, on an HTML element.
    assert_eq!(
        js(
            &mut engine,
            view,
            "var d = document.getElementById('d'); \
             for (var i = 0; i < 34; i++) d.setAttribute('data-b' + i, i); /* 35 with its id */ \
             for (var i = 0; i < 4; i++) d.removeAttribute('data-b' + i); \
             d.setAttribute('title', 'added'); \
             [d.getAttribute('title'), d.hasAttribute('title'), d.attributes.length, /title=\"added\"/.test(d.outerHTML)].join(' ')"
        ),
        "added true 32 true"
    );
    // The same on the shape, then a frame: the serializer runs again.
    js(
        &mut engine,
        view,
        "var r = document.getElementById('r'); for (var i = 0; i < 4; i++) r.removeAttribute('data-a' + i); \
         r.setAttribute('data-new', '1'); String(r.getBoundingClientRect().width)",
    );
    engine.mouse_move_at_point(view, 50.0, 50.0);
    assert_eq!(js(&mut engine, view, "document.getElementById('r').getAttribute('data-new')"), "1");
}

/// The live panic itself (a page thread died loading a real page: `no such
/// attribute` in the inline-SVG serializer). A shape with exactly 33
/// attributes and no `style`, two of them presentation attributes whose
/// author value is `inherit`: the serializer's copy drops those two (31
/// left, under the size the name set is kept from), then adds `style`,
/// which the stale set did not hold, then wrote every name it listed by
/// looking it up.
#[test]
fn an_svg_shape_whose_author_paint_is_inherit_serializes_with_its_style_added() {
    let mut attrs = String::new();
    for i in 0..28 {
        attrs.push_str(&format!(" data-a{i}=\"{i}\""));
    }
    // id, width, height, fill, stroke and 28 more: 33.
    let (mut engine, view) = page(&format!(
        "<!doctype html><style>#r {{ fill: inherit; stroke: inherit; stroke-width: 3px }}</style>\
         <body style=\"margin:0\"><svg id=s width=200 height=100 fill=green stroke=blue style=\"display:block\">\
         <rect id=r width=200 height=100 fill=red stroke=red{attrs}></rect></svg>"
    ));
    assert_eq!(js(&mut engine, view, "String(document.getElementById('r').attributes.length)"), "33");
    // The page laid out and painted, and still answers.
    engine.mouse_move_at_point(view, 50.0, 50.0);
    assert_eq!(js(&mut engine, view, "document.elementFromPoint(50, 50).id"), "r");
}

/// The trusted key and text events go to script through the host object
/// (`JsRuntime::evaluate_host_turn`), and the page's listeners run inside
/// that call: a listener that never returns is stopped by the turn's
/// budget and reported, as one in any other script turn is.
#[test]
fn a_listener_looping_forever_in_a_trusted_key_or_text_event_is_stopped_and_reported() {
    for event in ["keydown", "textInput"] {
        let config = EngineConfig { interrupt_scripts_at_budget: true, live_turn_budget_ms: 300, ..EngineConfig::default() };
        let mut engine = Engine::new(config).expect("engine");
        let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
        engine
            .load_html(
                view,
                &format!(
                    "<!doctype html><body style=\"margin:0\"><input id=f style=\"display:block\">\
                     <script>window.heard = 0; document.addEventListener('{event}', function () {{ heard++; while (true) {{}} }});</script>"
                ),
            )
            .expect("load");
        assert_eq!(engine.focus_at_point(view, 20.0, 8.0).as_deref(), Some("input"));
        let started = std::time::Instant::now();
        engine.handle_text_key(view, 0, "q", false, false, false);
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(5), "{event}: {took:?}");
        let log = engine.script_log(view).unwrap();
        assert!(
            log.iter().any(|r| r.outcome == ScriptOutcome::OverBudget && r.source == format!("event:{event}")),
            "{event}: {log:#?}"
        );
        // The listener ran once, and the page is usable afterwards.
        assert_eq!(js(&mut engine, view, "String(heard)"), "1", "{event}");
    }
}

/// MED 1: a timing, run by hand in the parity profile, before and after:
/// `cargo test -p rustkit-engine --lib --features headless --profile parity r1_svg_pointer_cost -- --ignored --nocapture`
///
/// A chart of 5,000 paths; the pointer moves 200 times over its background
/// (no shape is under it, so every shape is a candidate each time).
#[test]
#[ignore = "a timing, run by hand in the parity profile"]
fn r1_svg_pointer_cost() {
    let mut html = String::from("<!doctype html><body style=\"margin:0\"><svg id=s width=800 height=600 style=\"display:block\">");
    for i in 0..5000 {
        let (x, y) = (10 + (i % 100) * 7, 10 + (i / 100) * 9);
        html.push_str(&format!("<path d=\"M{x} {y} l 3 0 l 0 3 z\"></path>"));
    }
    html.push_str("</svg><script>var n = 0; document.addEventListener('mousemove', function (e) { n++; });</script>");
    let (mut engine, view) = page(&html);
    let started = std::time::Instant::now();
    for i in 0..200 {
        engine.mouse_move_at_point(view, 720.0 + (i % 10) as f32, 500.0 + (i % 7) as f32);
    }
    let moves = started.elapsed().as_secs_f64() * 1000.0;
    let started = std::time::Instant::now();
    let hits = js(&mut engine, view, "var h = 0; for (var i = 0; i < 200; i++) { if (document.elementFromPoint(720 + i % 10, 500 + i % 7).id === 's') h++; } String(h)");
    let reads = started.elapsed().as_secs_f64() * 1000.0;
    println!("5,000 paths: 200 pointer moves {moves:.1} ms ({:.3} ms each); 200 elementFromPoint {reads:.1} ms ({hits} on the svg)", moves / 200.0);
}

/// MED 2: a timing, run by hand in the parity profile, before and after:
/// `cargo test -p rustkit-engine --lib --features headless --profile parity r1_range_rect_cost -- --ignored --nocapture`
///
/// 5,000 paragraphs, one text node each; a range over all of them asks for
/// its bounding rect (what a selection toolbar does on `selectionchange`).
#[test]
#[ignore = "a timing, run by hand in the parity profile"]
fn r1_range_rect_cost() {
    let mut html = String::from("<!doctype html><body style=\"margin:0\"><div id=all>");
    for i in 0..5000 {
        html.push_str(&format!("<p style=\"margin:0\">paragraph {i}</p>"));
    }
    html.push_str("</div>");
    let (mut engine, view) = page(&html);
    js(&mut engine, view, "window.r = document.createRange(); r.selectNodeContents(document.getElementById('all')); 0");
    let started = std::time::Instant::now();
    let rect = js(&mut engine, view, "var b = r.getBoundingClientRect(); [b.width > 0, Math.round(b.height)].join(' ')");
    let bounding = started.elapsed().as_secs_f64() * 1000.0;
    let started = std::time::Instant::now();
    let count = js(&mut engine, view, "String(r.getClientRects().length)");
    let rects = started.elapsed().as_secs_f64() * 1000.0;
    println!("5,000 text nodes: getBoundingClientRect {bounding:.1} ms ({rect}); getClientRects {rects:.1} ms ({count} rects)");
}
