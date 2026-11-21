//! A click on a cookie-consent dialog must reach the dialog's listeners.
//!
//! Live report (2026-10-21): on a news site's consent dialog, drawn as it
//! should be, a click on "Agree" logged no `pointerdown`, `mousedown` or
//! `click` and the dialog stayed. The pages in `fixtures/consent-overlay`
//! are the shapes consent managers draw that dialog in. Each is clicked at
//! the centre of its button where it is painted (`agree_centre`: its
//! layout box under its ancestors' transforms, not
//! `getBoundingClientRect()`) through the press and release the app's
//! window click goes through.

use super::*;

const VIEW: Bounds = Bounds { x: 0, y: 0, width: 800, height: 600 };

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("../../../fixtures/consent-overlay/", $name))
    };
}

/// An engine with `page` loaded and its inline scripts run, or `None`
/// when there is no GPU to make one with.
fn page(html: &str) -> Option<(Engine, EngineViewId)> {
    let mut engine = match Engine::new(EngineConfig::default()) {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("Skipping test: no engine ({e:?})");
            return None;
        }
    };
    let id = engine.create_headless_view(VIEW).expect("headless view");
    engine.load_html(id, html).expect("load_html");
    let log = engine.script_log(id).expect("view");
    assert!(log.iter().all(|r| r.outcome == ScriptOutcome::Ran), "the page's script runs: {log:?}");
    Some((engine, id))
}

fn js(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let out = engine.execute_script(id, script).expect("script");
    // `JsValue::String("…")` as Debug: the text between the quotes.
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .map_or(out.clone(), str::to_string)
}

/// The centre of `window.__agree` where the window shows it: its box with
/// the transforms of it and its ancestors applied as the painter applies
/// them (`own_transform_affine`), moved by the scroll unless a fixed box
/// holds it to the window. `getBoundingClientRect()` is not used: it
/// leaves out transforms, and moves a fixed box with the scroll.
fn agree_centre(engine: &mut Engine, id: EngineViewId) -> (f32, f32) {
    js(engine, id, "__agree.setAttribute('data-agree', ''); ''");
    fn find(
        engine: &Engine,
        id: EngineViewId,
        b: &LayoutBox,
        m: [f32; 6],
        held: bool,
        transformed: bool,
    ) -> Option<(f32, f32, bool)> {
        let m = own_transform_affine(b).map_or(m, |own| compose_affine(m, own));
        let held = held || (b.position == Position::Fixed && !transformed);
        let transformed = transformed || !b.style.effective_transform().is_identity();
        let is_agree = b
            .node_id
            .and_then(|n| engine.node(id, n))
            .is_some_and(|n| n.get_attribute("data-agree").is_some());
        if is_agree {
            let r = b.dimensions.border_box();
            let (x, y) = (r.x + r.width / 2.0, r.y + r.height / 2.0);
            return Some((m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5], held));
        }
        b.children.iter().find_map(|c| find(engine, id, c, m, held, transformed))
    }
    let layout = engine.views[&id].layout.clone().expect("layout");
    let (x, y, held) = find(engine, id, &layout, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], false, false)
        .expect("Agree has a box");
    let scroll = engine.views[&id].scroll_offset;
    if held {
        (x, y)
    } else {
        (x - scroll.0, y - scroll.1)
    }
}

/// Press and release at the point, as the app does for a window click,
/// and return what the page's own listeners heard (`__heard`) and, for a
/// failure message, the target each pointer event went to.
fn click(engine: &mut Engine, id: EngineViewId, (x, y): (f32, f32)) -> (String, String) {
    js(
        engine,
        id,
        "window.__targets = []; \
         ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click'].forEach(function (t) { \
           window.addEventListener(t, function (e) { \
             var n = e.composedPath()[0] || e.target; \
             __targets.push(t + ':' + (n.id || n.className || n.nodeName)); }, true); });",
    );
    engine.mouse_down_at_point(id, x, y);
    engine.click_at_point(id, x, y);
    (
        js(engine, id, "__heard.join(' ')"),
        js(engine, id, "__targets.join(' ')"),
    )
}

/// Click the page's "Agree" and assert its listener heard `expected`.
fn agree_is_heard(html: &str, expected: &str) {
    let Some((mut engine, id)) = page(html) else { return };
    let at = agree_centre(&mut engine, id);
    let (heard, targets) = click(&mut engine, id, at);
    assert_eq!(
        heard, expected,
        "a click at {at:?} on Agree was not heard; the events went to: {targets}"
    );
}

#[test]
fn plain_dom_at_the_end_of_body() {
    agree_is_heard(fixture!("plain_dom.html"), "click");
}

#[test]
fn a_dialog_centred_with_translate() {
    agree_is_heard(fixture!("translate_centered.html"), "click");
}

#[test]
fn a_see_through_backdrop_with_a_clickable_dialog() {
    agree_is_heard(fixture!("backdrop_pointer_events_none.html"), "click");
}

#[test]
fn a_role_button_div_with_a_listener_delegated_to_the_document() {
    agree_is_heard(fixture!("delegated_on_document.html"), "click:accept");
}

#[test]
fn a_button_that_listens_for_pointerup_only() {
    agree_is_heard(fixture!("pointerup_only.html"), "pointerup:mouse");
}

#[test]
fn a_fixed_overlay_inside_a_transformed_ancestor() {
    agree_is_heard(fixture!("fixed_in_transformed_ancestor.html"), "click");
}

#[test]
fn a_fixed_overlay_in_a_zero_size_fixed_container() {
    agree_is_heard(fixture!("zero_size_fixed_wrapper.html"), "click");
}

#[test]
fn a_fixed_overlay_over_a_scrolled_page() {
    agree_is_heard(fixture!("scrolled_page.html"), "click");
}

#[test]
fn an_open_shadow_root_with_a_delegated_listener() {
    agree_is_heard(fixture!("shadow_open.html"), "click:agree");
}

#[test]
fn a_closed_shadow_root_retargets_to_its_host_for_the_document() {
    agree_is_heard(fixture!("shadow_closed.html"), "click document:cmp-host");
}

/// The engine has no nested browsing contexts: an `<iframe>` lays out as
/// an empty box, its document is never loaded and nothing in it is drawn
/// (`contentDocument` is null, web_reflect.js). A consent dialog in a frame
/// is not drawn, so it is not the live report, where the dialog was.
#[test]
#[ignore = "iframes have no nested browsing context yet: the frame's buttons are never loaded or drawn"]
fn a_dialog_inside_an_iframe() {
    let Some((mut engine, id)) = page(fixture!("iframe_srcdoc.html")) else { return };
    // Where the frame's button would be: centred, 200px + 24px padding +
    // a paragraph down.
    let (heard, targets) = click(&mut engine, id, (400.0, 280.0));
    assert_eq!(heard, "message:agree", "the events went to: {targets}");
}

