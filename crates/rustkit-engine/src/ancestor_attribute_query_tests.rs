//! CH2.E50: an attribute selector (and `:not()` of one) on an ANCESTOR
//! compound of a descendant or child selector is honoured by
//! `querySelector`/`matches`.
//!
//! `SelectorMatcher::node_matches` (the DOM bindings' matcher) built the
//! ancestor chain with attributes for the root element only, and an
//! ancestor that carries none leaves the compound's `[...]` and `:not()`
//! parts permissive. So `#tabs .tab[data-id="1"] .tab-title` matched every
//! tab's title and `querySelector` returned the first (found by M1b,
//! hiwave-macos #1009, in the app's own chrome).
//!
//! The expected column is what the pinned Chrome 148.0.7778.216 returns for
//! `querySelectorAll` on this page; a rule with the same selector colours
//! the same titles there (`getComputedStyle`), for every case.
//!
//! Selectors 4 §3 (complex selectors are matched right to left, each
//! compound in full); WPT `dom/nodes/ParentNode-querySelector-All.html`
//! and `css/selectors/attribute-selectors/` cover the same ground.

use super::*;

const PAGE: &str = r#"<!doctype html><html><head><style id="st"></style></head><body><div id="tabs"><div class="tab a" data-id="0" data-k="alpha-one" lang="en-US" id="t0"><span class="tab-title" id="s0">x</span></div><div class="tab b" data-id="1" data-k="beta two" lang="fr" id="t1"><span class="tab-title" id="s1">x</span></div><div class="tab a" data-id="2" data-k="gamma-three" id="t2" title="x"><span class="tab-title" id="s2">x</span></div></div></body></html>"#;

/// (selector, ids `querySelectorAll` returns in document order, as Chrome 148).
const CASES: &[(&str, &str)] = &[
    (r#"#tabs .tab[data-id="1"] .tab-title"#, "s1"),
    (r#".tab[data-id="2"] > .tab-title"#, "s2"),
    (r#"[data-id="0"] span"#, "s0"),
    (r#".tab[data-k^="beta"] span"#, "s1"),
    (r#".tab[data-k$="three"] span"#, "s2"),
    (r#".tab[data-k*="pha"] span"#, "s0"),
    (r#".tab[data-k~="two"] span"#, "s1"),
    (r#".tab[lang|="en"] span"#, "s0"),
    (r#".tab[data-id][title] span"#, "s2"),
    (r#".tab[data-id="1"][title] span"#, ""),
    (r#"[data-k="BETA TWO" i] span"#, "s1"),
    (r#".tab:not([data-id="1"]) .tab-title"#, "s0,s2"),
    (r#".tab:not(.b) > span"#, "s0,s2"),
    (r#"div:not(#t0) > .tab-title"#, "s1,s2"),
    (r#".tab.a .tab-title"#, "s0,s2"),
    (r#"#t1 .tab-title"#, "s1"),
    (r#".tab:is([data-id="0"], [data-id="2"]) span"#, "s0,s2"),
    (r#":not([data-id]) > .tab-title"#, ""),
    (r#"#tabs > [data-id="2"] > span"#, "s2"),
    (r#"[data-id="9"] span, #t1 > span"#, "s1"),
];

/// Cases where RustKit's style matching ALSO differs from Chrome, through
/// the cascade's own representation, not the query's: a structural
/// pseudo-class on an ancestor compound is permissive (the chain carries no
/// sibling position), and a sibling compound's attribute selectors are
/// permissive (`SiblingKey` carries no attributes). Not fixed by CH2.E50;
/// kept here, with Chrome 148's answers, for the follow-up row.
const CASCADE_GAPS: &[(&str, &str)] = &[
    (r#".tab:first-child .tab-title"#, "s0"),
    (r#".tab:nth-child(2) span"#, "s1"),
    (r#".tab:last-child span"#, "s2"),
    (r#".tab[data-id="1"] ~ .tab span"#, "s2"),
    (r#".tab[data-id="0"] + .tab span"#, "s1"),
];

fn loaded(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 }).expect("view");
    engine.load_html(view, html).expect("load_html");
    (engine, view)
}

fn read(engine: &mut Engine, view: EngineViewId, script: &str) -> String {
    let out = engine.execute_script(view, &format!("String(eval({script:?}))")).expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .unwrap_or_else(|| panic!("{script} evaluated to {out}"))
        .to_string()
}

fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

#[test]
fn query_selector_all_honours_ancestor_compound_filters() {
    let (mut engine, view) = loaded(PAGE);
    let mut wrong = Vec::new();
    for (selector, want) in CASES {
        let got = queried(&mut engine, view, selector);
        if got != *want {
            wrong.push(format!("{selector}: got [{got}], Chrome [{want}]"));
        }
    }
    assert!(wrong.is_empty(), "querySelectorAll differs from Chrome:\n{}", wrong.join("\n"));
}

/// The row's reproduction: `querySelector` with a tab's id returns THAT
/// tab's title, and `matches`/`closest` agree.
#[test]
fn query_selector_picks_the_tab_named_by_its_attribute() {
    let (mut engine, view) = loaded(PAGE);
    for n in 0..3 {
        let got = read(
            &mut engine,
            view,
            &format!(
                "var t = document.querySelector('#tabs .tab[data-id=\"{n}\"] .tab-title'); \
                 [t.id, t.closest('.tab').dataset.id, \
                  document.getElementById('s{n}').matches('.tab[data-id=\"{n}\"] > .tab-title'), \
                  document.getElementById('s{n}').matches('.tab:not([data-id=\"{n}\"]) > .tab-title')].join(' ')"
            ),
        );
        assert_eq!(got, format!("s{n} {n} true false"), "tab {n}");
    }
}

/// Which of the three titles a rule with `selector` colours, read with
/// `getComputedStyle`: the cascade's matcher.
fn styled(selector: &str) -> String {
    let page = PAGE.replace(
        "<style id=\"st\"></style>",
        &format!("<style id=\"st\">{selector} {{ color: rgb(0, 128, 0) }}</style>"),
    );
    let (mut engine, view) = loaded(&page);
    read(
        &mut engine,
        view,
        "['s0', 's1', 's2'].filter(function (id) { return getComputedStyle(document.getElementById(id)).color === 'rgb(0, 128, 0)'; }).join(',')",
    )
}

fn queried(engine: &mut Engine, view: EngineViewId, selector: &str) -> String {
    read(
        engine,
        view,
        &format!(
            "Array.from(document.querySelectorAll({})).map(function (e) {{ return e.id; }}).join(',')",
            js_string(selector)
        ),
    )
}

/// The cascade's matcher on the same cases: a rule with the selector
/// colours exactly the titles Chrome colours. It already carried ancestor
/// attributes, so this was green before the fix; it pins that the query and
/// the style keep agreeing.
#[test]
fn style_matching_agrees_with_chrome_on_the_same_cases() {
    let wrong: Vec<String> = CASES
        .iter()
        .filter_map(|(selector, want)| {
            let got = styled(selector);
            (got != *want).then(|| format!("{selector}: styled [{got}], Chrome [{want}]"))
        })
        .collect();
    assert!(wrong.is_empty(), "style matching differs from Chrome:\n{}", wrong.join("\n"));
}

/// The follow-up's red test: both the query and the style over-match.
#[test]
#[ignore = "CH2.E50 follow-up: structural pseudo-classes on ancestors, attributes on sibling compounds"]
fn cascade_gaps_match_chrome() {
    let (mut engine, view) = loaded(PAGE);
    let mut wrong = Vec::new();
    for (selector, want) in CASCADE_GAPS {
        let got = queried(&mut engine, view, selector);
        let style = styled(selector);
        if got != *want || style != *want {
            wrong.push(format!("{selector}: query [{got}], style [{style}], Chrome [{want}]"));
        }
    }
    assert!(wrong.is_empty(), "differs from Chrome:\n{}", wrong.join("\n"));
}
