//! `window.matchMedia` and `MediaQueryList` (web_match_media.js over the
//! rustkit-css evaluator). Its own test module so it does not collide with
//! the other families' tests in `lib.rs`.
//!
//! Expected values are what Chrome 148 answers for the same scripts.

use super::*;

const PAGE: &str = "<!DOCTYPE html><html><head><title>T</title></head><body><p>x</p></body></html>";

fn bound(width: f64, height: f64) -> DomBindings {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    bindings.set_dimensions(width, height).unwrap();
    bindings
}

fn ev(bindings: &DomBindings, script: &str) -> String {
    match bindings.evaluate(script) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Boolean(b)) => b.to_string(),
        Ok(JsValue::Number(n)) if n.fract() == 0.0 => format!("{}", n as i64),
        Ok(JsValue::Number(n)) => n.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(other) => panic!("{script} evaluated to {other:?}"),
        Err(e) => format!("THREW {e}"),
    }
}

/// A resize as the engine performs it: the new size, `resize` at window,
/// then the media query change report.
fn resize(bindings: &DomBindings, width: f64, height: f64) -> u32 {
    bindings.set_dimensions(width, height).unwrap();
    bindings.fire_lifecycle_event(LifecycleTarget::Window, "resize").unwrap();
    bindings.fire_media_query_changes().unwrap()
}

#[test]
fn min_width_answers_for_the_viewport_at_two_widths() {
    let b = bound(1280.0, 800.0);
    assert_eq!(ev(&b, "matchMedia('(min-width: 600px)').matches"), "true");
    assert_eq!(ev(&b, "matchMedia('(max-width: 599.98px)').matches"), "false");
    assert_eq!(ev(&b, "matchMedia('screen and (min-width: 960px)').matches"), "true");
    assert_eq!(ev(&b, "matchMedia('(min-height: 801px)').matches"), "false");
    assert_eq!(ev(&b, "matchMedia('(orientation: landscape)').matches"), "true");

    let b = bound(500.0, 900.0);
    assert_eq!(ev(&b, "matchMedia('(min-width: 600px)').matches"), "false");
    assert_eq!(ev(&b, "matchMedia('(max-width: 599.98px)').matches"), "true");
    assert_eq!(ev(&b, "matchMedia('screen and (min-width: 960px)').matches"), "false");
    assert_eq!(ev(&b, "matchMedia('(orientation: portrait)').matches"), "true");
}

#[test]
fn types_and_environment_features_answer_as_chrome_does() {
    let b = bound(1280.0, 800.0);
    assert_eq!(
        ev(&b, "['all', 'screen', 'print', 'not print', 'not screen', '', '(prefers-color-scheme: dark)', \
                '(prefers-color-scheme: light)', '(hover: hover)', '(pointer: coarse)', '(prefers-reduced-motion: reduce)']\
                .map(function (q) { return matchMedia(q).matches; }).join()"),
        "true,true,false,true,false,true,false,true,true,false,false"
    );
}

#[test]
fn media_is_the_serialized_query() {
    let b = bound(1280.0, 800.0);
    assert_eq!(ev(&b, "matchMedia('(min-width: 1px)').media"), "(min-width: 1px)");
    assert_eq!(ev(&b, "matchMedia('(MIN-WIDTH:600PX)').media"), "(min-width: 600px)");
    assert_eq!(ev(&b, "matchMedia('screen  and   (max-width: 640px)').media"), "screen and (max-width: 640px)");
    assert_eq!(ev(&b, "matchMedia('').media"), "");
    assert_eq!(ev(&b, "matchMedia('print, (min-width: 1000px)').media"), "print, (min-width: 1000px)");
}

#[test]
fn an_invalid_query_is_not_all_and_never_throws() {
    let b = bound(1280.0, 800.0);
    assert_eq!(ev(&b, "var q = matchMedia('foo bar baz'); [q.matches, q.media].join()"), "false,not all");
    assert_eq!(ev(&b, "var q = matchMedia('(min-width: 600px'); [q.matches, q.media].join()"), "false,not all");
    assert_eq!(ev(&b, "var q = matchMedia('not(hover)'); [q.matches, q.media].join()"), "false,not all");
    assert_eq!(ev(&b, "var q = matchMedia('screen, foo bar'); [q.matches, q.media].join()"), "true,screen, not all");
    // An invalid query under `not` is still false.
    assert_eq!(ev(&b, "matchMedia('not foo bar').matches"), "false");
    // A listener on it is accepted and never fires.
    assert_eq!(
        ev(&b, "var n = 0; var q = matchMedia('foo bar'); q.addEventListener('change', function () { n++; }); n"),
        "0"
    );
    assert_eq!(resize(&b, 300.0, 300.0), 0);
    assert_eq!(ev(&b, "[n, q.matches, q.media].join()"), "0,false,not all");
}

#[test]
fn the_list_is_a_media_query_list_event_target() {
    let b = bound(1280.0, 800.0);
    assert_eq!(
        ev(&b, "var q = matchMedia('(min-width: 1px)'); \
                [q instanceof MediaQueryList, q instanceof EventTarget, Object.prototype.toString.call(q), \
                 typeof q.addEventListener, typeof q.removeEventListener, typeof q.dispatchEvent, \
                 typeof q.addListener, typeof q.removeListener, q.onchange, \
                 Object.keys(q).length].join()"),
        "true,true,[object MediaQueryList],function,function,function,function,function,,0"
    );
    // The accessors live on the prototype and refuse a foreign receiver.
    assert_eq!(
        ev(&b, "try { Object.getOwnPropertyDescriptor(MediaQueryList.prototype, 'matches').get.call({}); 'no' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
    assert_eq!(ev(&b, "try { matchMedia(); 'no' } catch (e) { e.constructor.name }"), "TypeError");
    assert_eq!(ev(&b, "try { new MediaQueryList(); 'no' } catch (e) { e.constructor.name }"), "TypeError");
}

#[test]
fn matches_reads_the_viewport_as_it_stands_before_any_report() {
    let b = bound(1280.0, 800.0);
    b.evaluate("var q = matchMedia('(min-width: 600px)')").unwrap();
    assert_eq!(ev(&b, "q.matches"), "true");
    // The size changed and nothing has been reported yet: `matches` is
    // already the new answer (Chrome evaluates it on read).
    b.set_dimensions(500.0, 800.0).unwrap();
    assert_eq!(ev(&b, "q.matches"), "false");
    b.set_dimensions(1280.0, 800.0).unwrap();
    assert_eq!(ev(&b, "q.matches"), "true");
}

#[test]
fn change_fires_on_a_resize_that_flips_the_answer() {
    let b = bound(1280.0, 800.0);
    b.evaluate(
        "var seen = []; var q = matchMedia('(min-width: 600px)'); \
         q.addEventListener('change', function (e) { \
             seen.push([e.type, e.matches, e.media, e.target === q, this === q, \
                        e instanceof MediaQueryListEvent, q.matches, innerWidth].join(':')); \
         });",
    )
    .unwrap();

    // Narrower: flips to false.
    assert_eq!(resize(&b, 500.0, 800.0), 1);
    assert_eq!(ev(&b, "seen.join('|')"), "change:false:(min-width: 600px):true:true:true:false:500");

    // Narrower still: no flip, no event.
    assert_eq!(resize(&b, 400.0, 800.0), 0);
    assert_eq!(ev(&b, "seen.length"), "1");

    // Wide again: flips back.
    assert_eq!(resize(&b, 1024.0, 800.0), 1);
    assert_eq!(ev(&b, "seen[1]"), "change:true:(min-width: 600px):true:true:true:true:1024");

    // The same size announced again changes nothing.
    assert_eq!(resize(&b, 1024.0, 800.0), 0);
    assert_eq!(ev(&b, "seen.length"), "2");
    assert!(b.take_reported_errors().is_empty());
}

#[test]
fn resize_listeners_see_the_new_answer_and_change_follows_resize() {
    let b = bound(1280.0, 800.0);
    b.evaluate(
        "var order = []; var q = matchMedia('(min-width: 600px)'); \
         window.addEventListener('resize', function () { order.push('resize:' + q.matches); }); \
         q.onchange = function (e) { order.push('change:' + e.matches); };",
    )
    .unwrap();
    assert_eq!(resize(&b, 500.0, 800.0), 1);
    assert_eq!(ev(&b, "order.join()"), "resize:false,change:false");
}

#[test]
fn onchange_handler_and_legacy_add_listener_hear_change() {
    let b = bound(1280.0, 800.0);
    b.evaluate(
        "var seen = []; \
         var a = matchMedia('(max-width: 599px)'); \
         function la(e) { seen.push('legacy:' + e.matches + ':' + e.media); } a.addListener(la); \
         var o = matchMedia('(min-width: 600px)'); \
         o.onchange = function (e) { seen.push('onchange:' + e.matches); }; \
         var both = matchMedia('(min-width: 600px)'); \
         function h(e) { seen.push('both:' + e.matches); } \
         both.addListener(h); both.addEventListener('change', h); \
         var gone = matchMedia('(min-width: 600px)'); \
         function g2() { seen.push('gone'); } \
         gone.addListener(g2); gone.removeListener(g2); \
         var untouched = matchMedia('(min-width: 600px)');",
    )
    .unwrap();
    assert_eq!(ev(&b, "[String(a.onchange), o.onchange === null, typeof o.onchange].join()"), "null,false,function");
    // `addListener` registers the same listener as `addEventListener('change')`:
    // once. Three lists hear the report; `gone`, with nobody listening, is
    // dropped from the document's list at this report (Chrome collects such
    // lists too); `untouched` never was in it.
    assert_eq!(resize(&b, 500.0, 800.0), 3);
    assert_eq!(ev(&b, "seen.join('|')"), "legacy:true:(max-width: 599px)|onchange:false|both:false");
    assert_eq!(ev(&b, "[a.matches, o.matches, both.matches, gone.matches, untouched.matches].join()"), "true,false,false,false,false");

    // Removing the legacy listener silences it; `onchange = null` too. Both
    // lists are dropped at this report: only `both` hears it.
    b.evaluate("a.removeListener(la); o.onchange = null;").unwrap();
    assert_eq!(resize(&b, 1280.0, 800.0), 1);
    assert_eq!(ev(&b, "seen.slice(3).join('|')"), "both:true");
}

/// A list whose only listener was removed leaves the document's list of
/// media query lists at the next report (#818 R1 M2: React's useMediaQuery
/// churns listeners on a long-lived page, and a list nothing can hear has
/// no reason to stay; Chrome collects such lists too). One that listens
/// again joins afresh and compares from then.
#[test]
fn a_list_whose_only_listener_was_removed_is_dropped_at_the_next_report() {
    let b = bound(1280.0, 800.0);
    b.evaluate(
        "var n = 0; var q = matchMedia('(min-width: 600px)'); \
         function f() { n++; } q.addEventListener('change', f); \
         var o = matchMedia('(min-width: 600px)'); o.onchange = function () { n++; }; \
         var once = matchMedia('(min-width: 600px)'); \
         once.addEventListener('change', function () { n++; }, { once: true });",
    )
    .unwrap();
    // All three are live and hear the flip.
    assert_eq!(resize(&b, 500.0, 800.0), 3);
    assert_eq!(ev(&b, "n"), "3");

    // `q` lost its only listener, `o` its handler, `once` fired its listener
    // once: the next report drops all three, and none counts as hearing it.
    b.evaluate("q.removeEventListener('change', f); o.onchange = null;").unwrap();
    assert_eq!(resize(&b, 1280.0, 800.0), 0);
    assert_eq!(ev(&b, "n"), "3");
    // Dropped: a flip while nothing listens is not reported later either.
    assert_eq!(resize(&b, 500.0, 800.0), 0);

    // Listening again joins the list afresh: no flip since then, nothing to
    // report; the next flip is heard.
    b.evaluate("q.addEventListener('change', f);").unwrap();
    assert_eq!(resize(&b, 400.0, 800.0), 0);
    assert_eq!(ev(&b, "n"), "3");
    assert_eq!(resize(&b, 1024.0, 800.0), 1);
    assert_eq!(ev(&b, "[n, q.matches].join()"), "4,true");
    assert!(b.take_reported_errors().is_empty());
}

/// A non-ASCII char in a range query reached a byte-stepping walk in the
/// serializer (#818 R1 H1): `matchMedia('(ü>1)')` panicked the host closure.
/// The query is invalid, `not all` and false, and never throws.
#[test]
fn a_non_ascii_range_query_is_not_all_and_never_throws() {
    let b = bound(1280.0, 800.0);
    assert_eq!(ev(&b, "var q = matchMedia('(ü>1)'); [q.media, q.matches].join()"), "not all,false");
    assert_eq!(ev(&b, "var q = matchMedia('(width > 600pxé)'); [q.media, q.matches].join()"), "not all,false");
    assert_eq!(ev(&b, "matchMedia('screen, (ü > 1)').media"), "screen, not all");
    assert_eq!(ev(&b, "var q = matchMedia('(width > 600px)'); [q.media, q.matches].join()"), "(width > 600px),true");
    // A listener on it is accepted and never fires.
    assert_eq!(ev(&b, "var n = 0; var q = matchMedia('(ü>1)'); q.onchange = function () { n++; }; n"), "0");
    assert_eq!(resize(&b, 300.0, 300.0), 0);
    assert_eq!(ev(&b, "[n, q.matches, q.media].join()"), "0,false,not all");
}

#[test]
fn a_list_that_starts_listening_late_compares_from_then() {
    let b = bound(1280.0, 800.0);
    b.evaluate("var n = 0; var q = matchMedia('(min-width: 600px)');").unwrap();
    // The answer flipped while nobody listened.
    assert_eq!(resize(&b, 500.0, 800.0), 0);
    b.evaluate("q.addEventListener('change', function () { n++; });").unwrap();
    // No flip since the listener came: nothing to report.
    assert_eq!(resize(&b, 400.0, 800.0), 0);
    assert_eq!(ev(&b, "n"), "0");
    assert_eq!(resize(&b, 800.0, 800.0), 1);
    assert_eq!(ev(&b, "n"), "1");
}

#[test]
fn a_throwing_change_listener_is_reported_and_the_rest_still_hear() {
    let b = bound(1280.0, 800.0);
    b.evaluate(
        "var seen = []; var q = matchMedia('(min-width: 600px)'); \
         q.addEventListener('change', function () { throw new Error('boom'); }); \
         q.addEventListener('change', function (e) { seen.push(e.matches); }); \
         var r = matchMedia('(max-width: 599px)'); r.onchange = function (e) { seen.push('r:' + e.matches); };",
    )
    .unwrap();
    assert_eq!(resize(&b, 500.0, 800.0), 2);
    assert_eq!(ev(&b, "seen.join()"), "false,r:true");
    let errors = b.take_reported_errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("boom"), "{errors:?}");
}
