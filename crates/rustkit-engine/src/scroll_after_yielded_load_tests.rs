//! #730 blocker (yahoo.com, 10-22 Mac runs): after a load that yielded at
//! `DOMContentLoaded` and finished on live turns, the first wheel flick
//! down showed the content view mostly white below the first viewport, and
//! a second flick did not bring the content back.
//!
//! Cause: while the tail was open, live turns ran no timers (`pump_live`'s
//! `defer_post_load_timers`, 19ae9790), so a `scroll` listener that
//! debounces through `setTimeout` never filled the rows the user scrolled
//! to. After each wheel scroll the frame the presenter holds must paint the
//! rows inside the viewport.

use super::*;
use crate::error_page_script_tests::serve;

const W: u32 = 400;
const H: u32 = 300;

/// Fraction of the viewport's rows, at the commit's scroll offset, that a
/// non-white fill of the committed display list covers.
fn painted_rows(engine: &Engine, view: EngineViewId) -> (f32, (f32, f32), usize) {
    let commit = engine.latest_commit(view).expect("a commit");
    let list = commit.list.as_ref().expect("a display list");
    let top = commit.scroll_offset.1;
    let mut covered = vec![false; H as usize];
    for command in &list.commands {
        let (color, rect) = match command {
            rustkit_layout::DisplayCommand::SolidColor(color, rect) => (color, rect),
            rustkit_layout::DisplayCommand::RoundedRect { color, rect, .. } => (color, rect),
            _ => continue,
        };
        if color.a <= 0.0 || (color.r == 255 && color.g == 255 && color.b == 255) {
            continue;
        }
        let from = (rect.y - top).max(0.0) as usize;
        let to = ((rect.y + rect.height - top).min(H as f32)).max(0.0) as usize;
        for row in covered.iter_mut().take(to).skip(from) {
            *row = true;
        }
    }
    let n = covered.iter().filter(|c| **c).count();
    (n as f32 / H as f32, commit.scroll_offset, list.commands.len())
}

fn wheel(engine: &mut Engine, view: EngineViewId, dy: f32) {
    let _ = engine.scroll_view(view, 0.0, dy).expect("scroll");
    engine.render_changed_views();
}

fn assert_painted(engine: &Engine, view: EngineViewId, when: &str) {
    let (fraction, offset, commands) = painted_rows(engine, view);
    let max = engine.views[&view].max_scroll_offset;
    assert!(
        fraction > 0.9,
        "{when}: only {:.0}% of the viewport's rows are painted at offset {offset:?} \
         (max {max:?}, {commands} commands)",
        fraction * 100.0
    );
}

/// A feed whose rows below the fold are placeholders until the page's
/// `scroll` listener fills them, debounced through `setTimeout` as news
/// front pages do. The inserted script arrives late, so the load's tail is
/// still open while the user scrolls.
fn lazy_site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
    match path {
        "/slow.js" => {
            std::thread::sleep(std::time::Duration::from_millis(4000));
            (200, "text/javascript", vec![], "window.__slow = 1;".into())
        }
        _ => (
            200,
            "text/html",
            vec![],
            r#"<html><head><style>
body { margin: 0 }
#top { height: 300px; background: #208040 }
.slot { height: 100px; }
.filled { background: #3050a0 }
</style></head><body><div id="top">top</div><div id="feed"></div><script>
var feed = document.getElementById('feed');
for (var i = 0; i < 48; i++) {
  var slot = document.createElement('div');
  slot.className = 'slot';
  feed.appendChild(slot);
}
var pending = null;
document.addEventListener('scroll', function () {
  if (pending) return;
  pending = setTimeout(function () {
    pending = null;
    var slots = document.querySelectorAll('.slot');
    for (var i = 0; i < slots.length; i++) slots[i].className = 'slot filled';
  }, 50);
});
var s = document.createElement('script');
s.src = '/slow.js';
document.head.appendChild(s);
</script></body></html>"#
                .into(),
        ),
    }
}

/// The scroll listener's debounce timer is due while the yielded load's
/// tail still waits for a slow inserted script. It runs on the next live
/// turns, as in Chrome, where a timer does not wait for `window` `load`;
/// until then the rows the user scrolled to are white.
#[test]
fn a_scroll_listener_timer_runs_while_the_yielded_tail_is_open() {
    let server = serve(lazy_site);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: W, height: H })
        .expect("view");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let yielded = rt
        .block_on(engine.load_url_yielding(view, server.url("/")))
        .expect("load");
    assert_eq!(yielded, LoadYield::Yielded, "the slow script is still arriving");
    rt.block_on(engine.pump_live(view, 16));

    wheel(&mut engine, view, -(H as f32) * 1.5);
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        rt.block_on(engine.pump_live(view, 20));
        engine.render_changed_views();
    }
    assert!(engine.views[&view].load_tail.is_some(), "the tail is still open");
    assert_painted(&engine, view, "after the first flick, with the tail open");

    wheel(&mut engine, view, -(H as f32) * 1.5);
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        rt.block_on(engine.pump_live(view, 20));
        engine.render_changed_views();
    }
    assert_painted(&engine, view, "after the second flick");
}
