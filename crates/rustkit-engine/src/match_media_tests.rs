//! `window.matchMedia(query).matches` evaluates the query against the view.
//!
//! The binding was a stub that answered `matches: false` for every query
//! (rustkit-bindings `window_js`), so a script that picked its layout from a
//! media query always took the narrowest branch. netflix.com's hero form
//! is served with `data-issplitform="true"` (email field and button
//! stacked) and its script flips it to `"false"` (one row) once
//! `(min-width: …)` matches: Chrome 148 at 1280x800 shows the row, RustKit
//! the stacked form (Pete's hand test 10-27). CSS `@media` in sheets is
//! evaluated by the engine and is not affected.
//!
//! Expected values are what the pinned Chrome 148 answers in a 1280x800
//! viewport. Now `matchMedia` shares the `@media` evaluator (rustkit-css
//! `media.rs`, through rustkit-bindings `web_match_media`), and a resize
//! reports `change` to the lists whose answer flipped.

use super::*;

const QUERIES: &str = "[\
    '(min-width: 600px)', 'screen and (min-width: 960px)', '(min-width: 1280px)', '(min-width: 1281px)',\
    '(max-width: 599.98px)', '(orientation: landscape)', 'print', 'not print', 'all', '(min-height: 801px)'\
    ].map(function (q) { return q + '=' + window.matchMedia(q).matches; }).join(' ')";

const CHROME: &str = "(min-width: 600px)=true screen and (min-width: 960px)=true (min-width: 1280px)=true \
    (min-width: 1281px)=false (max-width: 599.98px)=false (orientation: landscape)=true print=false \
    not print=true all=true (min-height: 801px)=false";

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn match_media_answers_for_the_views_viewport() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 1280, 800))
        .expect("headless view");
    engine
        .load_html(id, "<!doctype html><html><body><p>x</p></body></html>")
        .expect("load_html");
    let value = engine.execute_script(id, QUERIES).expect("script");
    let got = value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value);
    assert_eq!(got, CHROME.split_whitespace().collect::<Vec<_>>().join(" "));
}

/// The same queries in a narrow, portrait view: the answers follow the
/// view, not a fixed board size.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn match_media_follows_the_views_size() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 500, 900))
        .expect("headless view");
    engine
        .load_html(id, "<!doctype html><html><body><p>x</p></body></html>")
        .expect("load_html");
    let value = engine.execute_script(id, QUERIES).expect("script");
    let got = value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value);
    assert_eq!(
        got,
        "(min-width: 600px)=false screen and (min-width: 960px)=false (min-width: 1280px)=false \
         (min-width: 1281px)=false (max-width: 599.98px)=true (orientation: landscape)=false print=false \
         not print=true all=true (min-height: 801px)=true"
    );
}

/// A resize of the view re-evaluates the page's media query lists and fires
/// `change` (after `resize`) at those whose answer flipped, with the new
/// `matches` and the `media` text; a legacy `addListener` hears it too.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_resize_reports_change_to_the_lists_that_flipped() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 1280, 800))
        .expect("headless view");
    engine
        .load_html(
            id,
            r#"<!doctype html><html><head><script>
var seen = [];
var wide = window.matchMedia('(min-width: 600px)');
var tall = window.matchMedia('(min-height: 801px)');
var bad = window.matchMedia('foo bar');
window.addEventListener('resize', function () { seen.push('resize:' + innerWidth + ':' + wide.matches); });
wide.addEventListener('change', function (e) { seen.push('wide:' + e.matches + ':' + e.media); });
tall.addListener(function (e) { seen.push('tall:' + e.matches); });
bad.addEventListener('change', function (e) { seen.push('bad:' + e.matches); });
</script></head><body><p>x</p></body></html>"#,
        )
        .expect("load_html");
    let read = |engine: &mut Engine| {
        let value = engine
            .execute_script(id, "[seen.join('|'), wide.matches, tall.matches, bad.matches, bad.media].join('~')")
            .expect("script");
        value
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&value)
            .to_string()
    };
    assert_eq!(read(&mut engine), "~true~false~false~not all");

    // Narrow and tall: both answers flip, the invalid query stays false.
    engine
        .resize_view(id, Bounds::new(0, 0, 500, 900))
        .expect("resize");
    assert_eq!(
        read(&mut engine),
        "resize:500:false|wide:false:(min-width: 600px)|tall:true~false~true~false~not all"
    );

    // Narrower still: nothing flips, nothing is reported.
    engine
        .resize_view(id, Bounds::new(0, 0, 400, 900))
        .expect("resize");
    assert_eq!(
        read(&mut engine),
        "resize:500:false|wide:false:(min-width: 600px)|tall:true|resize:400:false~false~true~false~not all"
    );

    // Back to the board size: both flip back.
    engine
        .resize_view(id, Bounds::new(0, 0, 1280, 800))
        .expect("resize");
    assert_eq!(
        read(&mut engine),
        "resize:500:false|wide:false:(min-width: 600px)|tall:true|resize:400:false|\
         resize:1280:true|wide:true:(min-width: 600px)|tall:false~true~false~false~not all"
    );
}
