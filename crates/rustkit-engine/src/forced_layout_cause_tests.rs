//! LAT7: a geometry read lays the page out again (forced layout) only when
//! script changed something a layout can see since the last one. A read on
//! an unchanged tree answers from the last layout; a write that changes
//! nothing (the same value again, a node in no document, a comment, a box
//! inside a `display: none` subtree) leaves the tree unchanged. Each task's
//! forced layouts are logged with what dirtied the tree and which read
//! forced it (`cause=<write>:<read>`) and how long they took (`ms=`).

use super::*;
use crate::relayout_cause_engine_tests::{capture_lines, field_ms};

fn engine_with(page: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    engine.load_html(view, page).expect("load_html");
    (engine, view)
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn forced(engine: &Engine, view: EngineViewId) -> u64 {
    engine.relayout_stats(view).unwrap().forced_layouts
}

fn relayouts(engine: &Engine, view: EngineViewId) -> u32 {
    engine.relayout_stats(view).unwrap().relayouts
}

const PAGE: &str = "<html><head><style>body{margin:0} .x{width:77px}</style></head><body>\
    <div id=a class='k' data-n=1 style='width:100px;height:20px'>text<!--c--></div>\
    <div id=h style='display:none'><p id=p>old</p></div></body></html>";

#[test]
fn reading_geometry_twice_with_no_write_lays_out_nothing() {
    let (mut engine, view) = engine_with(PAGE);
    let before = relayouts(&engine, view);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a');\
             window.__w = [a.offsetWidth, a.offsetWidth, a.getBoundingClientRect().width,\
                           a.clientWidth, a.scrollHeight, getComputedStyle(a).width, a.offsetTop].join();",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "100,100,100,100,20,100px,0");
    assert_eq!(forced(&engine, view), 0, "a read on a clean tree is a lookup");
    assert_eq!(relayouts(&engine, view), before, "and no relayout follows the task");
}

#[test]
fn writing_back_the_value_an_element_has_is_not_a_write() {
    let (mut engine, view) = engine_with(PAGE);
    let before = relayouts(&engine, view);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a'); var w = [];\
             a.style.width = a.style.width; w.push(a.offsetWidth);\
             a.className = a.className; w.push(a.offsetWidth);\
             a.classList.add('k'); w.push(a.offsetWidth);\
             a.setAttribute('data-n', a.getAttribute('data-n')); w.push(a.offsetWidth);\
             a.dataset.n = '1'; w.push(a.offsetWidth);\
             a.firstChild.data = a.firstChild.data; w.push(a.offsetWidth);\
             a.firstChild.nodeValue = 'text'; w.push(a.offsetWidth);\
             window.__w = w.join();",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "100,100,100,100,100,100,100");
    assert_eq!(forced(&engine, view), 0, "no write changed a value");
    assert_eq!(relayouts(&engine, view), before, "nothing for the end of the task either");
}

#[test]
fn a_comment_write_is_not_a_layout_write() {
    let (mut engine, view) = engine_with(PAGE);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a');\
             a.lastChild.data = 'changed'; window.__w = a.offsetWidth;",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "100");
    assert_eq!(forced(&engine, view), 0, "comments are never laid out");
}

#[test]
fn writes_to_nodes_in_no_document_are_not_writes() {
    let (mut engine, view) = engine_with(PAGE);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a'); var w = [];\
             var d = document.createElement('div'); d.setAttribute('class', 'x'); w.push(a.offsetWidth);\
             d.appendChild(document.createElement('span')); w.push(a.offsetWidth);\
             d.textContent = 'built aside'; w.push(a.offsetWidth);\
             d.innerHTML = '<b>more</b>'; d.style.width = '60px'; w.push(a.offsetWidth);\
             var f = document.createDocumentFragment(); f.appendChild(document.createElement('i')); w.push(a.offsetWidth);\
             window.__w = w.join(); window.__d = d;",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "100,100,100,100,100");
    assert_eq!(forced(&engine, view), 0, "a detached tree is in no layout");
    // Inserting it is a write: one forced layout, and the box is there.
    engine
        .execute_script(view, "document.body.appendChild(__d); window.__dw = __d.offsetWidth;")
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__dw"), "60");
    assert_eq!(forced(&engine, view), 1, "the insert dirtied the tree");
}

#[test]
fn a_node_moved_out_of_the_document_is_a_write() {
    // The move takes the node out of a laid-out tree: dirty, even though
    // its new parent is in no document.
    let (mut engine, view) = engine_with(PAGE);
    engine
        .execute_script(
            view,
            "var d = document.createElement('div');\
             var a = document.getElementById('a'); d.appendChild(a);\
             window.__h = a.offsetWidth;",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__h"), "0");
    assert_eq!(forced(&engine, view), 1);
}

#[test]
fn a_write_inside_a_display_none_subtree_does_not_force_a_layout() {
    let (mut engine, view) = engine_with(PAGE);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a'); var p = document.getElementById('p'); var w = [];\
             w.push(a.offsetWidth);\
             p.firstChild.data = 'new'; w.push(a.offsetWidth);\
             p.className = 'x'; w.push(a.offsetWidth);\
             p.appendChild(document.createElement('span')); w.push(a.getBoundingClientRect().width);\
             p.style.color = 'red'; w.push(a.offsetHeight);\
             window.__w = w.join();",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "100,100,100,100,20");
    assert_eq!(forced(&engine, view), 0, "nothing outside the hidden subtree moved");
    // The writes still reach the next layout: shown, the paragraph has them.
    engine
        .execute_script(
            view,
            "document.getElementById('h').style.display = 'block';\
             window.__p = [p.offsetWidth, p.textContent, getComputedStyle(p).color].join();",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__p"), "77,new,rgb(255, 0, 0)");
    assert_eq!(forced(&engine, view), 1, "showing the subtree is a write");
}

#[test]
fn a_write_that_can_show_a_hidden_subtree_still_forces_a_layout() {
    // `:empty` on the hidden element itself, and `:has()` on an ancestor,
    // let a write inside the subtree change whether it is shown.
    for (case, sheet, script, want) in [
        (
            "the hidden element's own children (:empty)",
            "#e:empty{display:none} #e{height:30px}",
            "document.getElementById('e').appendChild(document.createElement('i'));",
            "30",
        ),
        (
            ":has() on an ancestor",
            "#e{display:none} #e:has(.on){display:block;height:40px}",
            "document.getElementById('q').className = 'on';",
            "40",
        ),
        (
            "the hidden element's own attribute",
            "#e{height:50px} #e.off{display:none}",
            "document.getElementById('e').className = '';",
            "50",
        ),
    ] {
        let page = format!(
            "<html><head><style>body{{margin:0}} {sheet}</style></head><body>\
             <div id=w><div id=e class=off>{}</div></div></body></html>",
            if case.contains(":has") { "<span id=q></span>" } else { "" }
        );
        let (mut engine, view) = engine_with(&page);
        engine
            .execute_script(
                view,
                &format!(
                    "var w = document.getElementById('w'); window.__b = w.offsetHeight; {script} window.__h = w.offsetHeight;"
                ),
            )
            .unwrap();
        assert_eq!(read(&mut engine, view, "window.__b"), "0", "{case}: hidden at first");
        assert_eq!(read(&mut engine, view, "window.__h"), want, "{case}");
        assert_eq!(forced(&engine, view), 1, "{case}");
    }
}

#[test]
fn a_read_in_an_animation_frame_during_a_css_animation_forces_no_layout() {
    // Each animation tick lays the page out and publishes it; a frame
    // callback that only reads finds the tree clean.
    let page = "<html><head><style>body{margin:0}\
        @keyframes spin{from{transform:rotate(0deg)}to{transform:rotate(360deg)}}\
        #s{width:20px;height:20px;animation:spin 200ms linear infinite}</style></head><body>\
        <div id=s></div><div id=a style='width:100px;height:20px'></div>\
        <script>window.__reads = 0; var a = document.getElementById('a');\
        function f(){ window.__reads += a.offsetWidth > 0 ? 1 : 0; requestAnimationFrame(f); }\
        requestAnimationFrame(f);</script></body></html>";
    let server = crate::script_net_tests::serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    for _ in 0..8 {
        rt.block_on(engine.pump_live(view, 16));
    }
    let reads: u32 = read(&mut engine, view, "window.__reads").parse().unwrap();
    assert!(reads >= 4, "the frame callback ran: {reads}");
    assert_eq!(forced(&engine, view), 0, "animation ticks do not dirty the tree for script");
}

/// LAT7c: a page whose element `#s` runs a Web Animation of `keyframes`
/// while a frame callback evaluates `expr` for 10 frames.
/// Returns (reads that ran, forced layouts).
fn waapi_frames(keyframes: &str, expr: &str) -> (u32, u64) {
    let page = "<html><head><style>body{margin:0} #s{width:20px;height:20px;background:red}</style></head><body>\
        <div id=s></div><div id=a style='width:100px;height:20px'></div>\
        <script>var a = document.getElementById('a'), s = document.getElementById('s');</script></body></html>";
    let server = crate::script_net_tests::serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    engine
        .execute_script(view, &format!("s.animate({keyframes}, {{duration: 1000, iterations: Infinity}})"))
        .unwrap();
    for _ in 0..4 {
        rt.block_on(engine.pump_live(view, 16));
    }
    engine
        .execute_script(
            view,
            &format!(
                "window.__reads = 0; (function f(){{ requestAnimationFrame(function(){{ var v = {expr}; \
                 window.__reads += 1; if (window.__reads < 10) f(); }}); }})()"
            ),
        )
        .unwrap();
    let before = forced(&engine, view);
    for _ in 0..12 {
        rt.block_on(engine.pump_live(view, 16));
    }
    let reads: u32 = read(&mut engine, view, "window.__reads").parse().unwrap();
    (reads, forced(&engine, view) - before)
}

/// LAT7c: a Web Animations frame of a paint property (opacity, colour,
/// transform) changes nothing a box's offset or client size reads, so a
/// geometry read in a frame callback during it lays nothing out, as during
/// the same CSS animation. Chromium (CDP `LayoutCount`, 10 rAF frames each
/// reading `offsetWidth`): CSS opacity 0, WAAPI opacity 0, background-color
/// 0, transform 0; WAAPI width 10.
#[test]
fn a_read_in_an_animation_frame_during_a_web_animation_of_a_paint_property_forces_no_layout() {
    for keyframes in [
        "[{opacity: 0}, {opacity: 1}]",
        "[{backgroundColor: 'red'}, {backgroundColor: 'blue'}]",
        "[{transform: 'translateX(0px)'}, {transform: 'translateX(50px)'}]",
    ] {
        let (reads, forced) = waapi_frames(keyframes, "a.offsetWidth + a.clientHeight");
        assert!(reads >= 5, "{keyframes}: the frame callback ran: {reads}");
        assert_eq!(forced, 0, "{keyframes}: a paint-property animation frame leaves offset/client reads clean");
    }
}

/// LAT7c's other side: what the animated value does reach still lays out.
/// A width animation moves boxes; the computed opacity and a transformed
/// rect read the animated value (Chromium answers both at the current time).
#[test]
fn reads_that_see_a_web_animation_value_still_force_a_layout() {
    let (reads, forced) = waapi_frames("[{width: '20px'}, {width: '60px'}]", "a.offsetWidth");
    assert!(reads >= 5, "{reads}");
    assert!(forced >= 3, "a width animation moves boxes: {forced}");
    let (_, forced) = waapi_frames("[{opacity: 0}, {opacity: 1}]", "getComputedStyle(s).opacity");
    assert!(forced >= 3, "the computed opacity reads the animated value: {forced}");
    let (_, forced) = waapi_frames(
        "[{transform: 'translateX(0px)'}, {transform: 'translateX(50px)'}]",
        "s.getBoundingClientRect().left",
    );
    assert!(forced >= 3, "a transformed rect reads the animated value: {forced}");
    let (_, forced) = waapi_frames("[{opacity: 0}, {opacity: 1}]", "s.getBoundingClientRect().left");
    assert_eq!(forced, 0, "opacity moves no rect");
}

#[test]
fn a_real_write_then_a_read_forces_one_layout_and_logs_its_cause() {
    let (mut engine, view) = engine_with(PAGE);
    let lines = capture_lines("Forced layouts in the script task", || {
        engine
            .execute_script(
                view,
                "var a = document.getElementById('a'); a.style.width = '70px';\
                 window.__w = a.offsetWidth; window.__w2 = a.offsetWidth;",
            )
            .unwrap();
    });
    assert_eq!(read(&mut engine, view, "window.__w + ',' + window.__w2"), "70,70");
    assert_eq!(forced(&engine, view), 1, "one write, one forced layout, then lookups");
    assert_eq!(lines.len(), 1, "{lines:#?}");
    let line = &lines[0];
    assert!(line.contains("forced_layouts=1"), "{line}");
    assert!(line.contains("cause=style:offset"), "{line}");
    assert!(field_ms(line, "ms").is_some_and(|ms| ms > 0.0), "{line}");

    // Each kind of write and read is named.
    for (script, cause) in [
        ("a.setAttribute('title', 't'); a.getBoundingClientRect();", "attribute:rect"),
        ("a.className = 'k2'; getComputedStyle(a).width;", "class:computed_style"),
        ("a.firstChild.data = 'other'; a.clientWidth;", "text:client"),
        ("document.body.appendChild(document.createElement('div')); document.body.scrollHeight;", "insert:scroll_size"),
        // scrollTo reads the window's scroll state (its clamp) first.
        ("document.body.removeChild(document.body.lastChild); window.scrollTo(0, 10);", "remove:window_scroll"),
        ("a.innerHTML = '<b>x</b>'; a.offsetLeft;", "html:offset"),
    ] {
        let lines = capture_lines("Forced layouts in the script task", || {
            engine.execute_script(view, &format!("var a = document.getElementById('a'); {script}")).unwrap();
        });
        assert_eq!(lines.len(), 1, "{script}: {lines:#?}");
        assert!(lines[0].contains(&format!("cause={cause}")), "{script}: {}", lines[0]);
    }
}

#[test]
fn the_navigation_summary_carries_forced_layouts_and_their_time() {
    let (mut engine, view) = engine_with(PAGE);
    engine
        .execute_script(view, "var a = document.getElementById('a'); a.style.width = '70px'; a.offsetWidth;")
        .unwrap();
    let lines = capture_lines("relayout summary url=", || {
        engine.load_html(view, "<p>next</p>").expect("navigate away");
    });
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains(" forced_layouts=1 "), "{}", lines[0]);
    assert!(field_ms(&lines[0], "forced_ms").is_some_and(|ms| ms > 0.0), "{}", lines[0]);
}

/// The reduced page for the row's measurement: 1,000 `offsetWidth` reads,
/// each after writes that change nothing a layout sees (a style restated,
/// a text node given its own data, a hidden paragraph's text). Prints the
/// task's time; run with `--nocapture` to see it.
#[test]
fn a_thousand_reads_between_writes_that_change_nothing_lay_out_nothing() {
    let mut rows = String::new();
    for i in 0..200 {
        rows.push_str(&format!("<div class=r style='height:10px'>row {i}</div>"));
    }
    let page = format!(
        "<html><head><style>body{{margin:0}}</style></head><body>\
         <div id=a style='width:100px;height:20px'>text</div>{rows}\
         <div id=h style='display:none'><p id=p>old</p></div></body></html>"
    );
    let (mut engine, view) = engine_with(&page);
    let started = std::time::Instant::now();
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a'), t = a.firstChild;\
             var hp = document.getElementById('p').firstChild; var s = 0;\
             s += a.offsetWidth;\
             for (var i = 0; i < 1000; i++) {\
               a.style.width = a.style.width; t.data = t.data; hp.data = 'n' + i;\
               s += a.offsetWidth;\
             }\
             window.__s = s;",
        )
        .unwrap();
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    eprintln!("LAT7 reduced page: 1000 reads, forced_layouts={} task_ms={ms:.1}", forced(&engine, view));
    assert_eq!(read(&mut engine, view, "window.__s"), "100100");
    assert_eq!(forced(&engine, view), 0);
}
