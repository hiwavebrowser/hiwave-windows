//! Which font file an `@font-face` `src` list asks the network for, against
//! what the pinned Chrome requests for the same page
//! (tools/parity_oracle/font_face_src_cases.json, written by
//! font_face_src_log.mjs): a source whose `format()` or `tech()` the loader
//! cannot use is skipped for the next one. The choice is made before any
//! byte arrives, so the server here answers every font with junk.

use super::error_page_script_tests::{engine, serve};
use super::*;

fn cases() -> Vec<serde_json::Value> {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/font_face_src_cases.json")).unwrap();
    assert_eq!(data["chrome_version"].as_str(), Some("148.0.7778.216"));
    data["cases"].as_array().unwrap().clone()
}

/// `/case/<n>` is case n's page (the rule and a paragraph in its family, as
/// font_face_src_log.mjs builds it); every other path is a font that is none.
fn site(path: &str, _cookie: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
    let Some(n) = path.strip_prefix("/case/").and_then(|n| n.parse::<usize>().ok()) else {
        return (200, "application/octet-stream", vec![], "not a font".into());
    };
    let case = &cases()[n];
    let rule = match case["css"].as_str() {
        Some(css) => css.to_string(),
        None => format!("font-family: T; src: {};", case["src"].as_str().unwrap()),
    };
    (
        200,
        "text/html",
        vec![],
        format!("<!doctype html><html><head><style>@font-face {{ {rule} }} p {{ font-family: T; }}</style></head><body><p>text</p></body></html>"),
    )
}

#[test]
fn the_font_requested_for_a_src_list_is_the_one_chrome_requests() {
    let cases = cases();
    assert!(cases.len() >= 51, "the case file lost cases: {}", cases.len());
    let (mut wrong, mut gaps) = (Vec::new(), 0);
    for (n, case) in cases.iter().enumerate() {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome"].as_str().expect("run font_face_src_log.mjs --write");
        // A case with a `gap` is a known difference, pinned by its `engine`
        // answer so that it closes on purpose.
        let expected = match case.get("gap") {
            Some(_) => {
                gaps += 1;
                case["engine"].as_str().expect("a gap names the engine's answer")
            }
            None => chrome,
        };
        let server = serve(site);
        let (mut engine, view, rt) = engine();
        let page = format!("/case/{n}");
        rt.block_on(engine.load_url(view, server.url(&page))).expect("page");
        let fonts: Vec<String> =
            server.requests().into_iter().map(|(target, _)| target).filter(|target| *target != page).collect();
        let ours = fonts.join(" ");
        if ours != expected {
            wrong.push(format!("{name}\n    chrome: [{chrome}]\n    ours:   [{ours}]"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} cases differ from Chrome:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
    assert!(gaps <= 1, "the known differences grew to {gaps}");
}
