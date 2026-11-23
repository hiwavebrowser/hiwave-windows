//! ROOT-REM: `rem` is the root element's computed font-size (css-values-4 §6.1.1).
//!
//! The engine resolved `rem` against 16px wherever it met one: an element's and a
//! pseudo-element's font-size, line-height, layout lengths (width, padding, `calc()`), so a page
//! that sets `<html style="font-size: 62.5%">` (youtube.com) drew its `1.4rem` text at 22.4px
//! where Chrome draws 14px. `<html>`'s own font-size was not absolutized either, so
//! `getComputedStyle(html).fontSize` read 16px for a `62.5%` or `2em` root.
//!
//! Expected lines: docs/diagnostics/2026-11-21/root_rem_cft148.txt, from
//! tools/parity_oracle/root_rem_log.mjs in Chrome for Testing 148.0.7778.216 at 800x600: for each
//! root (`62.5%`, `10px`, `2em`, `2rem`), `window.report()` after load and `window.report2()`
//! after script sets the root to 20px (every `rem` user restyles). The root's own `2rem` is the
//! initial 16px's; a media query's `60rem` is 960px whatever the root (so `#m` stays 1px wide).

use super::*;

const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-21/root_rem_cft148.txt");
const PAGES: [(&str, &str); 4] = [
    ("pct", include_str!("../../../docs/diagnostics/2026-11-21/root_rem_pct.html")),
    ("px", include_str!("../../../docs/diagnostics/2026-11-21/root_rem_px.html")),
    ("em", include_str!("../../../docs/diagnostics/2026-11-21/root_rem_em.html")),
    ("rem", include_str!("../../../docs/diagnostics/2026-11-21/root_rem_rem.html")),
];

fn report(engine: &mut Engine, view: EngineViewId, call: &str) -> Vec<String> {
    let got = engine
        .execute_script(view, &format!("JSON.stringify({call}().split('\\n'))"))
        .expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got).to_string();
    let got: String = serde_json::from_str(&got).expect("a JSON string");
    serde_json::from_str(&got).expect("a JSON array")
}

/// Chrome's lines for one `[section]` of the receipt.
fn chrome_section(name: &str) -> Vec<&'static str> {
    let header = format!("[{name}]");
    CHROME
        .lines()
        .skip_while(|l| *l != header)
        .skip(1)
        .take_while(|l| !l.starts_with('['))
        .collect()
}

/// Lines still off, for a reason outside this row: the `::before` box of one 20px or 40px Menlo
/// glyph is 12.04 / 24.08 wide here and 12.05 / 24.09 in Chrome, which rounds an inline-block's
/// shrink-to-fit width up to its 1/64 px grid. A px-sized inline-block of the same glyph differs
/// the same way on develop. They must still DIFFER: one that matches comes off the list.
const KNOWN: &[(&str, &str)] = &[
    ("pct", "pseudo font-size: "),
    ("pct after", "pseudo font-size: "),
    ("px", "pseudo font-size: "),
    ("px after", "pseudo font-size: "),
    ("em after", "pseudo font-size: "),
    ("rem after", "pseudo font-size: "),
];

#[test]
fn rem_is_the_root_elements_font_size_everywhere() {
    let mut wrong = Vec::new();
    let mut compared = 0;
    for (name, page) in PAGES {
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
            .expect("view");
        engine.load_html(view, page).expect("load");
        for (section, call) in [(name.to_string(), "window.report"), (format!("{name} after"), "window.report2")] {
            let want = chrome_section(&section);
            assert_eq!(want.len(), 7, "[{section}] has 7 lines in the receipt");
            let got = report(&mut engine, view, call);
            for (i, w) in want.iter().enumerate() {
                compared += 1;
                let listed = KNOWN.iter().any(|(s, prefix)| *s == section && w.starts_with(prefix));
                match got.get(i) {
                    Some(h) if (h == w) != listed => {}
                    Some(h) if listed => {
                        wrong.push(format!("  [{section}] {h}\n    matches Chromium now: take it off KNOWN"))
                    }
                    Some(h) => wrong.push(format!("  [{section}] {h}\n    Chromium: {w}")),
                    None => wrong.push(format!("  [{section}] (missing)\n    Chromium: {w}")),
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} of {compared} lines differ:\n{}", wrong.len(), wrong.join("\n"));
}
