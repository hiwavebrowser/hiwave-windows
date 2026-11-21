//! HTML2: a noscript's text is written raw when the node's document is the
//! page's and the page runs script, wherever the node is: in the tree, on
//! its own, under a detached element or in a fragment. Its document follows
//! it through creation (`createElement` on another document), adoption
//! (`adoptNode`, and appending into another document's tree), import
//! (`importNode`) and cloning (a clone keeps the original's document);
//! removing a node keeps its document (Chrome for Testing 148).
//!
//! Expected lines: docs/diagnostics/2026-11-12/html2_noscript_owner_document_cft148.txt.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/html2_noscript_owner_document.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/html2_noscript_owner_document_cft148.txt");

/// Lines that still differ, by their start: each must still differ.
const KNOWN: &[&str] = &[];

#[test]
fn a_noscripts_text_is_raw_when_its_document_is_the_pages_wherever_the_node_is() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    let got = engine
        .execute_script(view, "JSON.stringify(window.report().split('\\n'))")
        .expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got).to_string();
    let got: String = serde_json::from_str(&got).expect("a JSON string");
    let got: Vec<String> = serde_json::from_str(&got).expect("a JSON array");
    let expected: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 23, "the Chrome report has 23 lines");
    assert_eq!(got.len(), expected.len(), "as many lines as Chrome reports: {got:#?}");
    let (mut wrong, mut healed) = (Vec::new(), Vec::new());
    for (g, e) in got.iter().zip(&expected) {
        let held = KNOWN.iter().any(|k| e.starts_with(k));
        if g != e && !held {
            wrong.push(format!("  {g}\n    Chromium: {e}"));
        } else if g == e && held {
            healed.push(e.to_string());
        }
    }
    assert!(healed.is_empty(), "these lines match now; take them off KNOWN: {healed:#?}");
    assert!(wrong.is_empty(), "{} of {} report lines differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}

const PATHS_PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/html2_owner_paths.html");
const PATHS_CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/html2_owner_paths_cft148.txt");

fn report_lines(page: &str) -> Vec<String> {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = engine
        .execute_script(view, "JSON.stringify(window.report().split('\\n'))")
        .expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got).to_string();
    let got: String = serde_json::from_str(&got).expect("a JSON string");
    serde_json::from_str(&got).expect("a JSON array")
}

/// R1 on #1108: the owner is carried on an attribute write and a text
/// write, on `createElementNS`, on `Range.createContextualFragment` and on
/// a clone of a `<template>`; raw text is for HTML elements only.
#[test]
fn owner_is_carried_on_data_writes_ns_creation_contextual_fragments_and_template_clones() {
    let got = report_lines(PATHS_PAGE);
    let expected: Vec<&str> = PATHS_CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 13, "the Chrome report has 13 lines");
    assert_eq!(got.len(), expected.len(), "{got:#?}");
    let wrong: Vec<String> = got
        .iter()
        .zip(&expected)
        .filter(|(g, e)| g != e)
        .map(|(g, e)| format!("  {g}\n    Chromium: {e}"))
        .collect();
    assert!(wrong.is_empty(), "{} of {} lines differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}

/// Every node's document, through 200 random sequences (fixed seed) of
/// 400 steps of create, insert, parse-then-move,
/// adopt, import, clone, attribute write, text write and remove across the
/// page's document, a DOMParser document, a createHTMLDocument document and
/// template contents. The expected document of each node is worked out
/// from the operations (the DOM's rules), never read from the engine; after
/// every step each node's `ownerDocument` must be it, and a noscript's
/// text must be raw exactly when that document is the page's.
#[test]
fn owner_follows_every_operation_in_random_sequences() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine
        .load_html(view, "<!doctype html><body><div id=host></div><template id=t></template>")
        .expect("load");
    let result = engine.execute_script(view, OWNER_SEQUENCES).expect("script");
    assert_eq!(result, "String(\"ok\")", "{result}");
}

const OWNER_SEQUENCES: &str = r#"(function () {
  var seed = 0x9e3779b9;
  function next(n) { seed = (seed * 1103515245 + 12345) >>> 0; return (seed >>> 8) % n; }
  var P = new DOMParser().parseFromString('<body></body>', 'text/html');
  var H = document.implementation.createHTMLDocument('');
  var docs = { page: document, P: P, H: H };
  var TEXT = 'a<b>&', SRC = '<noscript>&lt;i&gt;x</noscript>';
  for (var round = 0; round < 200; round++) {
    var tpl = document.createElement('template'); document.getElementById('host').appendChild(tpl);
    var label = new Map(), nodes = [], tDoc = null, parsedIn = new Map();
    function track(n, l) { if (!label.has(n)) nodes.push(n); label.set(n, l); }
    function each(n, f) { f(n); for (var c = n.firstChild; c; c = c.nextSibling) each(c, f); }
    function relabel(n, l) { each(n, function (x) { track(x, l); }); }
    var containers = [
      [document.getElementById('host'), 'page'], [P.body, 'P'], [H.body, 'H'], [tpl.content, 'T']
    ];
    containers.forEach(function (c) { track(c[0], c[1]); });
    function check(step) {
      for (var i = 0; i < nodes.length; i++) {
        var n = nodes[i], l = label.get(n), od = n.ownerDocument;
        if (l === 'T') {
          if (od === document || od === P || od === H || !od) return step + ': a template-contents node reads ' + (od === document ? 'the page' : od ? 'another document' : 'null');
          if (tDoc === null) tDoc = od; else if (od !== tDoc) return step + ': template-contents nodes read two documents';
        } else if (od !== docs[l]) {
          return step + ': a node of ' + l + ' reads ' + (od === document ? 'page' : od === P ? 'P' : od === H ? 'H' : 'another');
        }
        if (parsedIn.has(n) && n.firstChild && n.firstChild.nodeType === 3) {
          var want = parsedIn.get(n), got = n.firstChild.data;
          if (got !== want) return step + ': a parsed noscript holds ' + JSON.stringify(got) + ', not ' + JSON.stringify(want);
          var shown = l === 'page' ? want : want.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
          if (n.innerHTML !== shown) return step + ': a parsed noscript of ' + l + ' serializes its text as ' + n.innerHTML;
        }
        if (n.nodeType === 1 && n.localName === 'noscript' && n.namespaceURI === 'http://www.w3.org/1999/xhtml') {
          var raw = n.outerHTML.indexOf('a<b>') >= 0;
          if (n.textContent === TEXT && raw !== (l === 'page')) return step + ': a noscript of ' + l + ' is ' + (raw ? 'raw' : 'escaped');
        }
      }
      return null;
    }
    var names = ['page', 'P', 'H', 'T'];
    function docOf(l) { return l === 'T' ? tpl.content.ownerDocument : docs[l]; }
    for (var step = 0; step < 400; step++) {
      var op = next(10), pick = nodes.length ? nodes[next(nodes.length)] : null, what;
      // Bound the run: past 60 tracked nodes no more are made.
      if ((op === 0 || op === 3 || op === 4) && nodes.length > 60) op = 1 + 4 * next(2);
      if (op === 0 || !pick) {
        var l = names[next(3)], d = docs[l], kind = next(3);
        var n = kind === 0 ? d.createElement('noscript') : kind === 1 ? d.createElementNS('http://www.w3.org/1999/xhtml', 'noscript') : d.createElement('div');
        if (n.localName === 'noscript') n.textContent = TEXT;
        relabel(n, l); what = 'create on ' + l;
      } else if (op === 1) {
        var c = containers[next(containers.length)], parent = c[0];
        if (pick.nodeType !== 1 || pick === parent || pick.contains(parent) || containers.some(function (x) { return x[0] === pick; })) continue;
        parent.appendChild(pick); relabel(pick, label.get(parent)); what = 'insert into ' + c[1];
      } else if (op === 2) {
        if (pick.nodeType !== 1 || containers.some(function (x) { return x[0] === pick; })) continue;
        var l2 = names[next(3)]; docs[l2].adoptNode(pick); relabel(pick, l2); what = 'adopt by ' + l2;
      } else if (op === 3) {
        if (pick.nodeType !== 1 || containers.some(function (x) { return x[0] === pick; })) continue;
        var l3 = names[next(3)]; var copy = docs[l3].importNode(pick, true); relabel(copy, l3); what = 'import into ' + l3;
        var is = [], id = []; each(pick, function (x) { is.push(x); }); each(copy, function (x) { id.push(x); });
        for (var q = 0; q < id.length; q++) if (parsedIn.has(is[q])) parsedIn.set(id[q], parsedIn.get(is[q]));
      } else if (op === 4) {
        if (pick.nodeType !== 1 || containers.some(function (x) { return x[0] === pick; })) continue;
        var cl = pick.cloneNode(true), src = [], dst = [];
        each(pick, function (x) { src.push(x); }); each(cl, function (x) { dst.push(x); });
        for (var k = 0; k < dst.length; k++) {
          track(dst[k], label.get(src[k]) || label.get(pick));
          if (parsedIn.has(src[k])) parsedIn.set(dst[k], parsedIn.get(src[k]));
        }
        what = 'clone';
      } else if (op === 5) {
        if (pick.nodeType !== 1) continue;
        pick.setAttribute('data-x', String(step)); what = 'set an attribute';
      } else if (op === 6) {
        if (pick.nodeType !== 1 || pick.localName !== 'noscript') continue;
        pick.textContent = TEXT; parsedIn.delete(pick); relabel(pick, label.get(pick)); what = 'set text';
      } else if (op === 7) {
        if (pick.nodeType !== 1 || !pick.firstChild || pick.firstChild.nodeType !== 3) continue;
        pick.firstChild.data = TEXT; parsedIn.delete(pick); what = 'set text data';
      } else if (op === 8) {
        if (!pick.parentNode || containers.some(function (x) { return x[0] === pick; })) continue;
        pick.parentNode.removeChild(pick); what = 'remove';
      } else {
        // Parse then move: markup parsed in one document (innerHTML on a
        // holder that document made, or insertAdjacentHTML next to a node
        // of it), then the result moved into a container. The text is the
        // parsing document's (literal with scripting on, decoded with it
        // off); the document is the container's.
        var pl = names[next(3)], c = containers[next(containers.length)], made;
        if (next(2) === 0) {
          var holder = docs[pl].createElement('div'); holder.innerHTML = SRC; made = holder.firstChild;
          c[0].appendChild(made); what = 'parse in ' + pl + ', move into ' + c[1];
        } else {
          var mark = docOf(c[1]).createElement('div'); c[0].appendChild(mark); track(mark, c[1]);
          mark.insertAdjacentHTML('beforeend', SRC); made = mark.firstChild; pl = c[1];
          what = 'insertAdjacentHTML in ' + c[1];
        }
        relabel(made, c[1]);
        parsedIn.set(made, pl === 'page' ? '&lt;i&gt;x' : '<i>x');
      }
      var bad = check('round ' + round + ' step ' + step + ' (' + what + ')');
      if (bad) return bad;
    }
  }
  return 'ok';
})()"#;

const MOVE_PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/html2_parse_then_move.html");
const MOVE_CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/html2_parse_then_move_cft148.txt");

/// R1 on #1108 (second): markup parsed in one document and moved into
/// another: `insertAdjacentHTML` at each position and in another document,
/// innerHTML and outerHTML setters, parse then import or adopt, template
/// contents, shadow root innerHTML, a contextual fragment.
#[test]
fn parse_then_move_parses_with_the_parsing_documents_scripting_flag() {
    let got = report_lines(MOVE_PAGE);
    let expected: Vec<&str> = MOVE_CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 16, "the Chrome report has 16 lines");
    assert_eq!(got.len(), expected.len(), "{got:#?}");
    let wrong: Vec<String> = got
        .iter()
        .zip(&expected)
        .filter(|(g, e)| g != e)
        .map(|(g, e)| format!("  {g}\n    Chromium: {e}"))
        .collect();
    assert!(wrong.is_empty(), "{} of {} lines differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}

const OUTER_PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/html2_outer_html_setter.html");
const OUTER_CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/html2_outer_html_setter_cft148.txt");

/// R1 on #1108 (third): the `outerHTML` setter on an element of the page,
/// of another document, with no parent, under the document, under a
/// fragment, under an SVG element and in a table, and with an empty or
/// null value.
#[test]
fn outer_html_setter_replaces_the_element_as_chrome_does() {
    let got = report_lines(OUTER_PAGE);
    let expected: Vec<&str> = OUTER_CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 9, "the Chrome report has 9 lines");
    assert_eq!(got.len(), expected.len(), "{got:#?}");
    let wrong: Vec<String> = got
        .iter()
        .zip(&expected)
        .filter(|(g, e)| g != e)
        .map(|(g, e)| format!("  {g}\n    Chromium: {e}"))
        .collect();
    assert!(wrong.is_empty(), "{} of {} lines differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}
