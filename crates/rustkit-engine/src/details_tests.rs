//! `<details>` and `<summary>` boxes (Z row C10 (d)). HTML 15.3.9: the
//! first `summary` child is shown first, wherever it sits among the children;
//! a details with no `summary` child shows a default one reading "Details";
//! a closed details shows its summary only.
//!
//! The rows are what Chromium 143.0.7499.4 (the parity oracle) reports for each
//! hand-made body: `<details height> <#s top> <#p top> <#q top>` ("-" when
//! the element has no box); a closed details reads only `<height> <#s top>`,
//! because Chromium lays out its hidden content. Every line is 20px. Written
//! by `trench/z/tools/c10/details_gen.py` on the Atlas hub. The disclosure
//! triangle is not here: the engine draws no list markers yet.

use super::*;

const STYLE: &str = "body{margin:0;font:16px/20px monospace} p{margin:0} details,summary{list-style:none}";

const CASES: &[(&str, &str, &str)] = &[
    ("closed, summary first", r##"<details id=d><summary id=s>sum</summary><p id=p>body</p></details>"##, "20 0"),
    ("open, summary first", r##"<details id=d open><summary id=s>sum</summary><p id=p>body</p></details>"##, "40 0 20 -"),
    ("closed, summary last", r##"<details id=d><p id=p>body</p><summary id=s>sum</summary></details>"##, "20 0"),
    ("open, summary last", r##"<details id=d open><p id=p>body</p><summary id=s>sum</summary></details>"##, "40 0 20 -"),
    ("open, summary between", r##"<details id=d open><p id=p>body</p><summary id=s>sum</summary><div id=q>more</div></details>"##, "60 0 20 40"),
    ("open, two summaries", r##"<details id=d open><p id=p>body</p><summary id=s>one</summary><summary id=q>two</summary></details>"##, "60 0 20 40"),
    ("closed, two summaries", r##"<details id=d><summary id=s>one</summary><summary id=q>two</summary></details>"##, "20 0"),
    ("closed, no summary", r##"<details id=d><p id=p>body</p></details>"##, "20 -"),
    ("open, no summary", r##"<details id=d open><p id=p>body</p><div id=q>more</div></details>"##, "60 - 20 40"),
    ("closed, empty", r##"<details id=d></details>"##, "20 -"),
    ("open, empty", r##"<details id=d open></details>"##, "20 - - -"),
    ("closed, summary nested in a div", r##"<details id=d><div id=q><summary id=s>x</summary></div><p id=p>body</p></details>"##, "20 -"),
    ("open, summary nested in a div", r##"<details id=d open><p id=p>body</p><div id=q><summary id=s>x</summary></div></details>"##, "60 40 20 40"),
    ("open, text before the summary", r##"<details id=d open>text<summary id=s>sum</summary></details>"##, "40 0 - -"),
    ("closed, text before the summary", r##"<details id=d>text<summary id=s>sum</summary></details>"##, "20 0"),
    ("open, summary display none", r##"<details id=d open><p id=p>body</p><summary id=s style="display:none">x</summary></details>"##, "20 - 0 -"),
];

const READ: &str = "(function () { \
    var top = function (i) { var e = document.getElementById(i); if (!e) return '-'; var r = e.getBoundingClientRect(); return r.height ? Math.round(r.top) : '-'; }; \
    var d = document.getElementById('d'); \
    if (!d.hasAttribute('open')) { var s = document.getElementById('s'); return [Math.round(d.getBoundingClientRect().height), s && s.parentElement === d ? top('s') : '-'].join(' '); } \
    return [Math.round(d.getBoundingClientRect().height), top('s'), top('p'), top('q')].join(' '); })()";

#[test]
fn details_boxes_match_chromium() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds::new(0, 0, 800, 600))
        .expect("headless view");
    let mut wrong = Vec::new();
    for (name, body, want) in CASES {
        let html = format!("<!doctype html><style>{STYLE}</style>{body}");
        engine.load_html(view, &html).expect("load_html");
        let value = engine.execute_script(view, READ).expect("script");
        let got = value
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&value);
        if got != *want {
            wrong.push(format!("{name}: {got}, Chromium {want}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} wrong:\n{}",
        wrong.len(),
        CASES.len(),
        wrong.join("\n")
    );
}

/// The default summary paints its text. The rows above read heights only, and
/// a zero-width summary block (its style built from `Default`, not the initial
/// style) kept the height and painted nothing.
#[test]
fn the_default_summary_paints_details() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds::new(0, 0, 800, 600))
        .expect("headless view");
    let html = format!(
        "<!doctype html><style>{STYLE}</style><details id=d><p>hidden</p></details><details id=e open><p>shown</p></details>"
    );
    engine.load_html(view, &html).expect("load_html");
    let list = engine.views[&view].display_list.as_ref().expect("display list");
    let texts: Vec<&str> = list
        .commands
        .iter()
        .filter_map(|cmd| match cmd {
            rustkit_layout::DisplayCommand::Text { text, .. } => Some(text.trim()),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect();
    assert_eq!(texts, ["Details", "Details", "shown"]);
}
