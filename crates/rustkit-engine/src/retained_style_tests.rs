//! TREE-REUSE (a): a view keeps each element's style from one build to the next across script
//! writes, and a build replays the style of every element the writes did not reach
//! (`docs/design/INCREMENTAL_RESTYLE.md` §3.3 item 1, PR 2).
//!
//! Step 0 (`hidden_tiles_build_tests`): on MotionMark Multiply's shape a full build costs ~11 us
//! per tile, hidden or shown, because every tile's own cascade runs (its inline style is unique,
//! so no two tiles share a style). The stage shows and hides tiles by writing `display`, which is
//! not a paint-only write, so each such frame builds the whole stage.
//!
//! A replayed style must be the style a full cascade gives: every test here runs the same writes
//! with retained styles on and off and compares the display lists and the computed styles of every
//! element, hidden ones included, after every frame.

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
    // Frame stamps on the frame grid, as parity-capture takes them: no real time reaches the
    // document timeline, so two runs of a page sample a running animation at the same times and
    // their display lists can be compared frame by frame (#1231).
    let config = EngineConfig {
        grid_frame_stamps: true,
        ..EngineConfig::default()
    };
    let mut engine = Engine::new(config).expect("engine");
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

fn frame(engine: &mut Engine, view: EngineViewId) {
    rt().block_on(engine.pump_live(view, 17));
}

/// `body` with an animation-frame loop that runs `write` (with `n` the frame number) each frame
/// until `window.STOP`. The load itself runs a few hundred frames, so `n` starts large.
fn page(style: &str, body: &str, write: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0;font:14px sans-serif}}{style}</style>{body}\
         <script>var n = 0; requestAnimationFrame(function f() {{ n++; if (!window.STOP) {{ {write} }} requestAnimationFrame(f); }});</script>"
    )
}

/// Multiply's shape: `tiles` tiles, each with its own inline position and colour, the first
/// `shown` visible and the rest `display: none`.
fn tiles(tiles: usize, shown: usize, write: &str) -> String {
    let mut body = String::from("<div id=stage>");
    for i in 0..tiles {
        body.push_str(&format!(
            "<div class=tile id=t{i} style=\"left:{}px;top:{}px;background-color:hsl({},80%,50%){}\"></div>",
            (i % 40) * 20,
            (i / 40 % 30) * 20,
            i % 360,
            if i < shown { "" } else { ";display:none" }
        ));
    }
    body.push_str("</div>");
    page(
        "#stage{position:relative;width:800px;height:600px;overflow:hidden}\
         .tile{position:absolute;width:18px;height:18px;border-radius:4px}",
        &body,
        write,
    )
}

/// The stage's tune: each frame shows one more tile and hides the oldest shown one, and moves a
/// shown tile (a geometry write, so no frame takes the paint-only path).
const TUNE: &str = "var N = document.querySelectorAll('.tile').length; \
                    var a = document.getElementById('t' + (10 + n) % N), b = document.getElementById('t' + n % N), \
                    c = document.getElementById('t' + (n + 5) % N); \
                    a.style.display = ''; b.style.display = 'none'; c.style.left = (n * 3 % 700) + 'px';";

/// Every element's computed style (a few properties of each kind), as script reads it.
const DUMP: &str = "(function () { var out = []; var all = document.querySelectorAll('*'); \
    for (var i = 0; i < all.length; i++) { var s = getComputedStyle(all[i]); \
    out.push([all[i].tagName, all[i].id, s.display, s.color, s.fontSize, s.left, s.marginTop, \
    s.backgroundColor, s.width].join('|')); } return out.join('\\n'); })()";

/// What one run of a page shows: the display list after each frame, every element's computed
/// style at the end, and the element cascades and element builds the frames ran.
struct Run {
    lists: Vec<String>,
    styles: String,
    cascades: usize,
    built: usize,
    /// Builds spliced into the kept box tree, and why the last that could did not.
    splices: (usize, String),
}

/// [`Run`] of `frames` frames with retained styles `mode`. The page writes in its first
/// `frames - 2` frames only (`window.STOP`), so the last frame's relayout takes the last writes and
/// script reads the styles of a clean document: a read between frames would force a build of its
/// own, which retained styles do not serve.
fn run(page: &str, frames: usize, mode: RetainedStyles) -> Run {
    set_retained_styles_for_test(Some(mode));
    let (mut engine, view, _server) = load(page);
    let (mut lists, mut cascades, mut built) = (Vec::new(), 0, 0);
    let spliced = splices_for_test().0;
    for i in 0..frames {
        if i == frames - 2 {
            engine
                .execute_script(view, "window.STOP = true")
                .expect("stop");
        }
        let (c, b) = (cascades_for_test(), built_elements_for_test());
        frame(&mut engine, view);
        cascades += cascades_for_test() - c;
        built += built_elements_for_test() - b;
        lists.push(format!(
            "{:?}",
            engine.views[&view]
                .display_list
                .as_ref()
                .expect("display list")
                .commands
        ));
    }
    let before = cascades_for_test();
    let styles = format!("{:?}", engine.execute_script(view, DUMP).expect("dump"));
    assert_eq!(
        cascades_for_test(),
        before,
        "the read of a clean document built nothing"
    );
    set_retained_styles_for_test(None);
    let (now, miss) = splices_for_test();
    Run {
        lists,
        styles,
        cascades,
        built,
        splices: (now - spliced, miss),
    }
}

/// The three arms give the same display lists and computed styles, and the retained arms did their
/// work: the styles arm replayed styles and the kept-boxes arm spliced into its kept tree. (How many
/// elements each arm built is no measure: the arms need not relayout the same number of times.)
fn assert_same_as_a_full_cascade(name: &str, page: &str) {
    assert_same_as_a_full_build(name, page, Some(true))
}

/// [`assert_same_as_a_full_cascade`]; with `splices` `Some(false)` the kept-boxes arms must not
/// splice (their relayouts build in full: while animations run, a kept box would hold last frame's
/// values), and with `None` they may or may not (a transition that may have ended before the
/// frames measured). Either way boxes-verify must find no splice that differs from a full build.
fn assert_same_as_a_full_build(name: &str, page: &str, splices: Option<bool>) {
    let full = run(page, 8, RetainedStyles::Off);
    let styles = run(page, 8, RetainedStyles::On);
    let boxes = run(page, 8, RetainedStyles::Boxes);
    // The gate's arm: each splice is compared with a full build beside it.
    let mismatches = splice_mismatches_for_test();
    let checked = run(page, 8, RetainedStyles::BoxesVerify);
    let mismatches = splice_mismatches_for_test() - mismatches;
    assert_eq!(
        mismatches, 0,
        "{name}: splices boxes-verify found different from a full build"
    );
    assert!(
        splices.is_none_or(|splices| (checked.splices.0 > 0) == splices),
        "{name}: the boxes-verify run spliced {} builds (last miss {:?})",
        checked.splices.0,
        checked.splices.1
    );
    // Not vacuous: the retained runs replayed styles, and the kept-boxes run spliced.
    assert!(
        styles.cascades < full.cascades,
        "{name}: retained styles ran {} cascades, a full cascade {}",
        styles.cascades,
        full.cascades
    );
    assert!(
        splices.is_none_or(|splices| (boxes.splices.0 > 0) == splices),
        "{name}: the kept-boxes run spliced {} builds, expected {} (built {} elements; last miss {:?})",
        boxes.splices.0,
        match splices {
            Some(true) => "some",
            Some(false) => "none",
            None => "any",
        },
        boxes.built,
        boxes.splices.1
    );
    for (arm, got) in [
        ("retained styles", &styles),
        ("kept boxes", &boxes),
        ("boxes-verify", &checked),
    ] {
        for i in 0..full.lists.len() {
            assert!(
                got.lists[i] == full.lists[i],
                "{name}, {arm}, frame {i}: the display list differs from a full build's"
            );
        }
        assert!(
            got.styles == full.styles,
            "{name}, {arm}: computed styles differ from a full build's
  {arm}: {}
  full: {}",
            got.styles,
            full.styles
        );
    }
}

/// PR 3's red test: with the box tree kept too, the same tune frame builds the written tiles (and
/// the path down to them), not the stage's 1,000 tiles.
#[test]
fn a_tune_frame_with_kept_boxes_builds_the_written_tiles_alone() {
    set_retained_styles_for_test(Some(RetainedStyles::Boxes));
    let (mut engine, view, _server) = load(&tiles(1_000, 10, TUNE));
    for _ in 0..3 {
        frame(&mut engine, view);
    }
    let mut per_frame = Vec::new();
    for _ in 0..5 {
        let before = built_elements_for_test();
        frame(&mut engine, view);
        per_frame.push(built_elements_for_test() - before);
    }
    set_retained_styles_for_test(None);
    assert!(
        per_frame.iter().all(|&b| b > 0 && b <= 20),
        "elements built per frame (3 tiles written of 1,000): {per_frame:?}"
    );
}

/// The red test: on Multiply's shape (1,000 tiles, 10 shown), a frame that shows a tile, hides
/// one and moves one cascades those tiles, not the stage (it cascaded all 1,000).
#[test]
fn a_frame_that_shows_and_hides_tiles_cascades_those_tiles_alone() {
    set_retained_styles_for_test(Some(RetainedStyles::On));
    let (mut engine, view, _server) = load(&tiles(1_000, 10, TUNE));
    for _ in 0..3 {
        frame(&mut engine, view);
    }
    let mut per_frame = Vec::new();
    for _ in 0..5 {
        let before = cascades_for_test();
        frame(&mut engine, view);
        per_frame.push(cascades_for_test() - before);
    }
    set_retained_styles_for_test(None);
    assert!(
        per_frame.iter().all(|&c| c > 0 && c <= 20),
        "element cascades per frame (3 tiles written of 1,000): {per_frame:?}"
    );
}

#[test]
fn tiles_shown_hidden_and_moved_keep_a_full_cascades_styles() {
    assert_same_as_a_full_cascade("tiles", &tiles(200, 10, TUNE));
}

/// For a page whose animations run while it is measured. Since #1231 the animation clock is the
/// document timeline; with real-time frame stamps two runs of one page sample a running animation
/// at different times (two plain full runs gave colours one step apart). `load` puts the stamps on
/// the frame grid, so the runs are compared frame by frame ([`assert_same_as_a_full_build`]), and
/// the check is also made inside one run, which no clock can upset: `verify` cascades every element
/// afresh beside its replay, and no replay that would have served may differ. The run must have
/// animated: its frames' display lists differ from each other. The kept box tree is
/// checked the same way: `boxes-verify` compares each splice with a full build beside it, and no
/// splice may differ; `splices` is as in [`assert_same_as_a_full_build`].
fn assert_replays_match_a_fresh_cascade(name: &str, page: &str, splices: Option<bool>) {
    // On the frame grid (`load`) the runs are comparable too: every arm's display lists and styles
    // match a full build's, frame by frame (#1246 delta R1 follow-up).
    assert_same_as_a_full_build(name, page, splices);
    let before = retained_mismatches_for_test();
    let verified = run(page, 8, RetainedStyles::Verify);
    assert_eq!(
        retained_mismatches_for_test() - before,
        0,
        "{name}: replays that would have served differ from a fresh cascade"
    );
    assert!(
        verified.lists.windows(2).any(|w| w[0] != w[1]),
        "{name}: nothing animated in the frames measured"
    );
    let before = splice_mismatches_for_test();
    let checked = run(page, 8, RetainedStyles::BoxesVerify);
    assert_eq!(
        splice_mismatches_for_test() - before,
        0,
        "{name}: splices boxes-verify found different from a full build"
    );
    assert!(
        splices.is_none_or(|splices| (checked.splices.0 > 0) == splices),
        "{name}: the boxes-verify run spliced {} builds (last miss {:?})",
        checked.splices.0,
        checked.splices.1
    );
}

/// R1 item 1: a parent whose inherited colour is mid-transition re-cascades to the same end value
/// every frame. Its children inherit the interpolated colour, which moves each frame, so they must
/// not replay last frame's. Another element is written each frame, so every relayout is script's.
/// The class flips every 4 frames, so a transition (2 s) is always running, reversed mid-way.
#[test]
fn a_running_transition_on_an_inherited_property_reaches_the_children() {
    assert_replays_match_a_fresh_cascade("transition", &transition_page(), None);
}

fn transition_page() -> String {
    page(
        "#p{color:rgb(0,0,255);transition:color 2s linear} #p.on{color:rgb(255,0,0)} #w{width:10px}",
        "<div id=p><span id=s>x<em id=e>y</em></span></div><div id=w></div>",
        "document.getElementById('p').className = n % 8 < 4 ? 'on' : ''; \
         document.getElementById('w').style.width = (10 + n % 50) + 'px';",
    )
}

/// #1257 delta R1: `boxes-verify` must see a splice that sweeps the animation state like a full
/// build. That bug (fixed in #1257) dropped the before-change style of every element the splice
/// did not visit, so the next class change started no transition; `boxes-verify` hid it, because
/// its comparison build visits every element inside the same `with_building`. With the fix undone
/// (`IGNORE_LAST_BUILD_PARTIAL`), the verify run must count splice differences.
#[test]
fn boxes_verify_counts_a_splice_that_sweeps_like_a_full_build() {
    crate::layout_ctx::IGNORE_LAST_BUILD_PARTIAL.with(|c| c.set(true));
    let before = splice_mismatches_for_test();
    let checked = run(&transition_page(), 8, RetainedStyles::BoxesVerify);
    let found = splice_mismatches_for_test() - before;
    crate::layout_ctx::IGNORE_LAST_BUILD_PARTIAL.with(|c| c.set(false));
    assert!(checked.splices.0 > 0, "the verify run spliced (last miss {:?})", checked.splices.1);
    assert!(found > 0, "boxes-verify found no difference in a splice that swept like a full build");
}

/// The same for an `@keyframes` animation of an inherited property on an ancestor.
#[test]
fn a_running_animation_on_an_inherited_property_reaches_the_children() {
    assert_replays_match_a_fresh_cascade(
        "keyframes",
        &page(
            "@keyframes pulse{from{color:rgb(255,0,0)} to{color:rgb(0,0,255)}} \
             #p{animation:pulse 1s linear infinite} #w{width:10px}",
            "<div id=p><span id=s>x<em id=e>y</em></span></div><div id=w></div>",
            "document.getElementById('w').style.width = (10 + n % 50) + 'px';",
        ),
        Some(false),
    );
}

/// R1 item 2: verify mode counts only replays that would have served an element. On a page whose
/// writes change an inherited value (children re-cascade because their parent changed, not because
/// a replay was wrong) it reports nothing.
#[test]
fn verify_counts_only_replays_that_would_have_served() {
    let page = page(
        ".a{color:rgb(200,0,0);font-size:12px} .b{color:rgb(0,0,200);font-size:20px}",
        "<div id=p class=a><p id=q>x<span id=s>y<em id=e>z</em></span></p></div><p id=r>w</p>",
        "document.getElementById('p').className = n % 2 ? 'b' : 'a';",
    );
    let before = retained_mismatches_for_test();
    let verified = run(&page, 8, RetainedStyles::Verify);
    assert!(verified.cascades > 0, "the verify run built");
    assert_eq!(
        retained_mismatches_for_test() - before,
        0,
        "replays a verify run reported wrong"
    );
}

#[test]
fn an_inherited_change_reaches_the_descendants() {
    assert_same_as_a_full_cascade(
        "inherited",
        &page(
            ".a{color:rgb(200,0,0);font-size:12px} .b{color:rgb(0,0,200);font-size:20px} em{margin-top:1em;display:block}",
            "<div id=p class=a><p id=q>x<span id=s>y<em id=e>z</em></span></p></div><p id=r>w</p>",
            "document.getElementById('p').className = n % 2 ? 'b' : 'a';",
        ),
    );
}

#[test]
fn a_sibling_selector_follows_its_sibling() {
    assert_same_as_a_full_cascade(
        "sibling",
        &page(
            ".on + p{color:rgb(0,128,0);margin-top:9px} .on ~ div{width:40px}",
            "<p id=a>a</p><p id=b>b</p><div id=c>c</div><div id=d>d</div>",
            "document.getElementById('a').className = n % 2 ? 'on' : '';",
        ),
    );
}

#[test]
fn a_structural_pseudo_class_follows_an_insert_and_a_removal() {
    assert_same_as_a_full_cascade(
        "nth-child",
        &page(
            "li:nth-child(2n){color:rgb(255,0,0)} li:first-child{margin-top:7px} li:last-child{width:50px;display:block}",
            "<ul id=u><li id=l1>1</li><li id=l2>2</li><li id=l3>3</li><li id=l4>4</li></ul>",
            "var u = document.getElementById('u'); if (n % 2) { var li = document.createElement('li'); \
             li.textContent = 'new'; u.insertBefore(li, u.firstChild); } else { u.removeChild(u.firstChild); }",
        ),
    );
}

#[test]
fn an_attribute_on_an_ancestor_restyles_its_descendants() {
    assert_same_as_a_full_cascade(
        "attribute",
        &page(
            "[data-k='1'] span{margin-top:5px;display:inline-block;color:rgb(9,9,9)}",
            "<div id=p><p><span id=s>x</span></p></div><span id=t>y</span>",
            "document.getElementById('p').setAttribute('data-k', String(n % 2));",
        ),
    );
}

#[test]
fn a_has_rule_follows_its_descendant() {
    assert_same_as_a_full_cascade(
        "has",
        &page(
            ".box{width:100px} .box:has(.on){background-color:rgb(255,0,0);width:60px}",
            "<div class=box id=x><p><i id=i>i</i></p></div><div class=box id=y></div>",
            "document.getElementById('i').className = n % 2 ? 'on' : '';",
        ),
    );
}

#[test]
fn a_custom_property_write_reaches_its_readers() {
    assert_same_as_a_full_cascade(
        "custom property",
        &page(
            "#r{--c:rgb(1,2,3);--w:10px} p{color:var(--c);width:var(--w)}",
            "<div id=r><p id=a>a</p><div><p id=b>b</p></div></div>",
            "var r = document.getElementById('r'); r.style.setProperty('--c', n % 2 ? 'rgb(9,8,7)' : 'rgb(1,2,3)'); \
             r.style.setProperty('--w', (10 + n) + 'px');",
        ),
    );
}

#[test]
fn a_hidden_elements_style_answers_after_its_write() {
    assert_same_as_a_full_cascade(
        "hidden",
        &page(
            ".h{display:none;color:rgb(0,0,0)} .h span{font-size:30px}",
            "<div class=h id=h><span id=s>x</span></div><div id=v>v</div>",
            "var h = document.getElementById('h'); h.style.color = 'rgb(' + n + ',0,0)'; h.style.left = n + 'px'; \
             h.style.display = n % 3 ? 'none' : 'block';",
        ),
    );
}

/// Prints `RETAINED-TILES n mode build_ms` (median over 9 frames of Multiply's tune, after 3).
#[test]
#[ignore]
fn measure_a_tune_frames_build_with_and_without_retained_styles() {
    for &n in &[1_000usize, 4_000, 8_000] {
        for mode in [
            RetainedStyles::Off,
            RetainedStyles::On,
            RetainedStyles::Boxes,
        ] {
            set_retained_styles_for_test(Some(mode));
            let (mut engine, view, _server) = load(&tiles(n, 10, TUNE));
            for _ in 0..3 {
                frame(&mut engine, view);
            }
            let mut ms = Vec::new();
            for _ in 0..9 {
                frame(&mut engine, view);
                ms.push(engine.relayout_stats(view).expect("view").last_build_ms);
            }
            set_retained_styles_for_test(None);
            ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
            eprintln!(
                "RETAINED-TILES n {n} {mode:?} build_ms {:.2}",
                ms[ms.len() / 2]
            );
        }
    }
}

#[test]
fn an_insert_and_a_removal_among_block_siblings_splice() {
    assert_same_as_a_full_cascade(
        "block siblings",
        &page(
            "#u>div{height:10px} #u>div:nth-child(2n){width:30px;background-color:rgb(255,0,0)} \
             #u>div:first-child{margin-top:7px} #u>div:last-child{width:50px}",
            "<div id=u><div id=d1></div><div id=d2></div><div id=d3></div><div id=d4></div></div><p id=after>p</p>",
            "var u = document.getElementById('u'); if (n % 2) { var d = document.createElement('div'); \
             u.insertBefore(d, u.firstChild); } else { u.removeChild(u.firstChild); }",
        ),
    );
}

/// #1247 R1 item 1: a list item that is shown or hidden renumbers the items after it, and the dirty
/// set never marks them (a counter is not a selector). A box that comes or goes among list items is
/// not spliced. (RustKit draws no list markers and no `counter()` yet, so the display lists agree
/// either way; the refusal is what keeps them agreeing once it does.)
#[test]
fn a_list_item_shown_and_hidden_builds_in_full() {
    assert_same_as_a_full_build(
        "list item shown and hidden",
        &page(
            "li{height:10px}",
            "<ol id=o><li id=a>a</li><li id=b>b</li><li id=c>c</li></ol><p id=after>p</p>",
            "document.getElementById('a').style.display = n % 2 ? 'none' : '';",
        ),
        Some(false),
    );
}

/// An item inserted into or removed from a list: the write marks the list, so the splice builds the
/// whole list afresh (its root is the `<ol>`) and every item is numbered as a full build numbers it.
#[test]
fn an_insert_among_list_items_rebuilds_the_list() {
    assert_same_as_a_full_build(
        "list insert",
        &page(
            "li{height:10px}",
            "<ol id=o><li id=a>a</li><li id=b>b</li><li id=c>c</li></ol><p id=after>p</p>",
            "var o = document.getElementById('o'); if (n % 2) { var li = document.createElement('li'); \
             li.textContent = 'new'; o.insertBefore(li, o.firstChild); } else { o.removeChild(o.firstChild); }",
        ),
        None,
    );
}

/// #1247 R1 item 1, counters: block siblings that number themselves with `counter-increment` and
/// show the count in `::before`. The counter properties and `counter()` generate nothing in RustKit
/// yet (`rustkit_css::content::content_string`), so this pins that a splice still agrees with a full
/// build here; `spliced_build` says what must refuse it once counters are drawn.
#[test]
fn an_insert_among_counted_siblings_matches_a_full_build() {
    assert_same_as_a_full_build(
        "counters",
        &page(
            "#u{counter-reset:c} #u>div{height:10px;counter-increment:c} #u>div::before{content:counter(c) '.'}",
            "<div id=u><div id=d1></div><div id=d2></div><div id=d3></div></div>",
            "var u = document.getElementById('u'); if (n % 2) { var d = document.createElement('div'); \
             u.insertBefore(d, u.firstChild); } else { u.removeChild(u.firstChild); }",
        ),
        None,
    );
}

/// #1247 R1 item 2: an animation that ran while the kept tree was built and has ended since leaves
/// its value on boxes no write reaches. `#a` animates its opacity for 100 ms from one write and is not
/// written again; `#w` is written every frame except for the 15 after that write, so the animation
/// runs and ends in frames with no script relayout. Every splice after it must agree with a full
/// build, and the frames after it do splice.
#[test]
fn an_animation_that_ended_since_the_kept_build_leaves_no_value_behind() {
    set_retained_styles_for_test(Some(RetainedStyles::BoxesVerify));
    let (mut engine, view, _server) = load(&page(
        "@keyframes fade{from{opacity:0.2} to{opacity:0.8}} #a{width:50px;height:50px;background-color:rgb(0,0,255)} \
         #a.go{animation:fade 100ms linear} #w{width:10px;height:5px}",
        "<div id=a></div><div id=w></div>",
        "if (window.GO && !window.WENT) { document.getElementById('a').className = 'go'; window.WENT = n; } \
         if (!window.WENT || window.WENT == n || n - window.WENT > 15) { \
         document.getElementById('w').style.width = (10 + n % 50) + 'px'; }",
    ));
    for _ in 0..3 {
        frame(&mut engine, view);
    }
    engine.execute_script(view, "window.GO = true").expect("go");
    let (mismatches, splices) = (splice_mismatches_for_test(), splices_for_test().0);
    for _ in 0..30 {
        frame(&mut engine, view);
    }
    let (now, miss) = splices_for_test();
    set_retained_styles_for_test(None);
    assert_eq!(
        splice_mismatches_for_test() - mismatches,
        0,
        "splices after the animation differ from a full build"
    );
    assert!(
        now > splices,
        "the frames after the animation ended splice (last miss {miss:?})"
    );
}

/// #1247 delta R1: a script animation (`element.animate`) is applied in the build from the view's
/// script layer, and can start, run and fill in frames no build sees. `#a` is animated once to
/// opacity 0 (filled, 100 ms) in a frame with no other write, and `#w`, written every frame before,
/// is written again only 15 frames later: every splice must agree with a full build, and the frames
/// after the animation has filled splice again.
#[test]
fn a_script_animation_started_after_the_kept_build_leaves_no_value_behind() {
    set_retained_styles_for_test(Some(RetainedStyles::BoxesVerify));
    let (mut engine, view, _server) = load(&page(
        "#a{width:50px;height:50px;background-color:rgb(0,0,255)} #w{width:10px;height:5px}",
        "<div id=a></div><div id=w></div>",
        "if (window.GO && !window.WENT) { document.getElementById('a').animate([{opacity:1},{opacity:0}], \
         {duration:100, fill:'forwards'}); window.WENT = n; } \
         if (!window.WENT || n - window.WENT > 15) { \
         document.getElementById('w').style.width = (10 + n % 50) + 'px'; }",
    ));
    for _ in 0..3 {
        frame(&mut engine, view);
    }
    engine.execute_script(view, "window.GO = true").expect("go");
    let (mismatches, _) = (splice_mismatches_for_test(), ());
    for _ in 0..20 {
        frame(&mut engine, view);
    }
    let splices = splices_for_test().0;
    for _ in 0..5 {
        frame(&mut engine, view);
    }
    let (now, miss) = splices_for_test();
    set_retained_styles_for_test(None);
    assert_eq!(
        splice_mismatches_for_test() - mismatches,
        0,
        "splices around a script animation differ from a full build"
    );
    assert!(
        now > splices,
        "the frames after the animation filled splice (last miss {miss:?})"
    );
}

/// TREE-REUSE step (1): a task that writes and then reads geometry. The read forces a full build;
/// the end-of-task relayout built the whole document again (`no kept tree`: the forced build let the
/// kept tree go). With the forced build's tree kept, the relayout splices into it: one full build
/// per frame, not two.
const TUNE_THEN_READ: &str = "var N = document.querySelectorAll('.tile').length; \
    var a = document.getElementById('t' + (10 + n) % N), b = document.getElementById('t' + n % N), \
    c = document.getElementById('t' + (n + 5) % N); \
    a.style.display = ''; b.style.display = 'none'; c.style.left = (n * 3 % 700) + 'px'; \
    window.R = c.offsetLeft;";

#[test]
fn a_tune_frame_that_reads_geometry_builds_the_document_once() {
    set_retained_styles_for_test(Some(RetainedStyles::Boxes));
    let (mut engine, view, _server) = load(&tiles(1_000, 10, TUNE_THEN_READ));
    for _ in 0..3 {
        frame(&mut engine, view);
    }
    let mut per_frame = Vec::new();
    for _ in 0..5 {
        let before = built_elements_for_test();
        frame(&mut engine, view);
        per_frame.push(built_elements_for_test() - before);
    }
    let miss = splices_for_test().1;
    set_retained_styles_for_test(None);
    // One full build walks the stage's 1,002 elements; the splice adds the written tiles.
    assert!(
        per_frame.iter().all(|&b| b > 1_000 && b <= 1_030),
        "elements built per frame (a forced read, then the relayout): {per_frame:?} (last miss {miss:?})"
    );
}

#[test]
#[cfg_attr(windows, ignore = "flaky on a loaded runner: the arms start at different writes; fixed upstream by hiwave-macos #1339, which needs #1304")]
fn tiles_written_then_read_keep_a_full_builds_boxes() {
    assert_same_as_a_full_cascade("tiles then read", &tiles(200, 10, TUNE_THEN_READ));
}

/// R1 item 5: the memo holds a style per element. After a stretch of relayouts that script writes
/// did not ask for, the view lets it go and records none; script writes start it again.
#[test]
fn a_stretch_without_script_relayouts_lets_the_styles_go() {
    set_retained_styles_for_test(Some(RetainedStyles::On));
    let (mut engine, view, _server) = load(&tiles(200, 10, TUNE));
    frame(&mut engine, view);
    frame(&mut engine, view);
    let held = |engine: &Engine| {
        engine.views[&view]
            .retained_styles
            .as_ref()
            .map_or(0, |m| m.styles.len())
    };
    assert!(
        held(&engine) > 200,
        "script relayouts keep a style per element: {}",
        held(&engine)
    );
    engine
        .execute_script(view, "window.STOP = true")
        .expect("stop");
    for _ in 0..12 {
        engine.relayout(view).expect("relayout");
    }
    assert_eq!(
        held(&engine),
        0,
        "styles held after 12 relayouts script did not ask for"
    );
    engine
        .execute_script(view, "window.STOP = false")
        .expect("go");
    for _ in 0..3 {
        frame(&mut engine, view);
    }
    set_retained_styles_for_test(None);
    assert!(
        held(&engine) > 200,
        "script relayouts keep them again: {}",
        held(&engine)
    );
}
