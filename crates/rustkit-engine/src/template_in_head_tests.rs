//! A `<template>` in `<head>` stays in the head, with what follows it
//! (HTML §13.2.6.4.4 "in head": a template start tag pushes "in
//! template"; its end tag pops back to "in head"). The tree builder had no
//! template case "in head": the head closed, and the template and every
//! head element after it went into the body (Speedometer's Perf-Dashboard:
//! two elements short of Chrome's document in every test).
//!
//! Expected lines: docs/diagnostics/2026-11-13/template_in_head_cft148.txt,
//! `window.report()` of the page beside it in Chrome for Testing
//! 148.0.7778.216 (each case parsed with DOMParser). Two cases from the R1:
//! formatting opened in a template stays in it (a marker), and a template
//! started "in body" takes its own mode, so its end tag leaves the outer
//! template's mode alone.
//! From the second R1: a raw text element the "in head" rules opened from
//! another mode (`<noframes>` in and after a frameset and in a template,
//! `<style>` in a table in a template) keeps its text and its end tag.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-13/template_in_head.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-13/template_in_head_cft148.txt");

#[test]
fn a_template_in_head_stays_in_head_with_what_follows_it() {
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let id = engine.create_page_view(Bounds::new(0, 0, 800, 600)).expect("page view");
    engine.load_html(id, PAGE).expect("load_html");
    let out = engine.execute_script(id, "String(window.report())").expect("script");
    let got = out
        .strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .map(|s| s.replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\"))
        .unwrap_or(out);
    let got: Vec<&str> = got.lines().collect();
    let want: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).collect();
    assert_eq!(got.len(), want.len(), "lines: {got:#?}");
    let wrong: Vec<String> = got.iter().zip(&want).filter(|(g, w)| g != w).map(|(g, w)| format!("got  {g}\n     want {w}")).collect();
    assert!(wrong.is_empty(), "the lines that differ from Chrome 148:\n{}", wrong.join("\n"));
}
