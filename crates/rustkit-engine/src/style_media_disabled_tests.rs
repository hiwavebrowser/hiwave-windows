//! CH2.E17: a `<style media>` applies only where its media query list
//! matches, and `<style>.sheet.disabled = true` takes the sheet out of the
//! cascade. Before this, both were no-ops: `<style media=print>` applied on
//! screen and a disabled sheet kept applying.
//!
//! Every expected value is Chromium 143.0.7499.4 on the same markup at an
//! 800x600 viewport (`scratch/e17_oracle.mjs` in the PR, hub receipt
//! `ch2-e17-oracle.mjs`). `#t` is 10px wide by the first sheet and 20px by
//! the second when that one applies.

use super::*;

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

const BASE: &str = "<style>body{margin:0} #t{width:10px}</style>";

fn page(body: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let html = format!("<html><head>{BASE}</head><body>{body}<div id=t></div></body></html>");
    engine.load_html(view, &html).expect("load_html");
    (engine, view)
}

/// `#t`'s offsetWidth x offsetHeight after `script` (if any).
fn size(body: &str, script: &str) -> String {
    let (mut engine, view) = page(body);
    if !script.is_empty() {
        engine.execute_script(view, script).expect("script");
    }
    let w = read(&mut engine, view, "document.getElementById('t').offsetWidth");
    let h = read(&mut engine, view, "document.getElementById('t').offsetHeight");
    format!("{w}x{h}")
}

fn media(list: &str) -> String {
    size(&format!("<style media=\"{list}\">#t{{width:20px}}</style>"), "")
}

#[test]
fn a_style_media_list_applies_only_where_it_matches() {
    let cases = [
        ("print", "10x0"),
        ("screen", "20x0"),
        ("all", "20x0"),
        ("", "20x0"),
        ("(min-width: 700px)", "20x0"),
        ("(min-width: 900px)", "10x0"),
        ("print, screen", "20x0"),
        ("foo bar", "10x0"),
        ("tv", "10x0"),
        ("SCREEN", "20x0"),
        ("not print", "20x0"),
    ];
    let mut wrong = Vec::new();
    for (list, want) in cases {
        let got = media(list);
        if got != want {
            wrong.push(format!("media={list:?}: got {got}, Chromium {want}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn an_inner_media_rule_cannot_widen_the_elements_media() {
    // media=print around `@media screen { ... }`: both must hold.
    assert_eq!(size("<style media=print>@media screen { #t{width:20px} }</style>", ""), "10x0");
}

#[test]
fn a_media_attribute_set_or_removed_by_script_applies() {
    let set = size("<style id=s>#t{width:20px}</style>", "document.getElementById('s').setAttribute('media','print')");
    assert_eq!(set, "10x0", "media=print set by script");
    let removed = size("<style id=s media=print>#t{width:20px}</style>", "document.getElementById('s').removeAttribute('media')");
    assert_eq!(removed, "20x0", "media removed by script");
}

#[test]
fn a_style_media_query_follows_a_viewport_resize() {
    let (mut engine, view) = page("<style media=\"(max-width: 500px)\">#t{width:20px}</style>");
    assert_eq!(read(&mut engine, view, "document.getElementById('t').offsetWidth"), "10");
    engine
        .resize_view(view, Bounds { x: 0, y: 0, width: 400, height: 600 })
        .expect("resize");
    assert_eq!(read(&mut engine, view, "document.getElementById('t').offsetWidth"), "20");
}

#[test]
fn a_disabled_style_sheet_leaves_the_cascade() {
    let s = "<style id=s>#t{width:20px}</style>";
    let cases = [
        ("sheet.disabled", "document.getElementById('s').sheet.disabled = true", "10x0"),
        ("styleSheets[1]", "document.styleSheets[1].disabled = true", "10x0"),
        (
            "disabled then enabled",
            "var s = document.getElementById('s').sheet; s.disabled = true; document.body.offsetWidth; s.disabled = false",
            "20x0",
        ),
        (
            "insertRule keeps it disabled",
            "var e = document.getElementById('s'); e.sheet.disabled = true; e.sheet.insertRule('#t{height:5px}', 1)",
            "10x0",
        ),
    ];
    let mut wrong = Vec::new();
    for (name, script, want) in cases {
        let got = size(s, script);
        if got != want {
            wrong.push(format!("{name}: got {got}, Chromium {want}"));
        }
    }
    let first = size(
        "",
        "var e = document.createElement('style'); e.id = 'x'; e.textContent = '#t{width:20px}'; document.head.appendChild(e);\
         document.querySelector('style').sheet.disabled = true",
    );
    if first != "20x0" {
        wrong.push(format!("first sheet disabled: got {first}, Chromium 20x0"));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn new_text_makes_a_new_enabled_sheet() {
    // Chromium makes a new CSSStyleSheet when a <style>'s text changes,
    // and a new sheet is not disabled.
    let s = "<style id=s>#t{width:20px}</style>";
    let changed = size(s, "var e = document.getElementById('s'); e.sheet.disabled = true; e.textContent = '#t{width:30px}'");
    assert_eq!(changed, "30x0", "textContent replaced");
    let appended = size(
        s,
        "var e = document.getElementById('s'); e.sheet.disabled = true; e.appendChild(document.createTextNode('#t{height:5px}'))",
    );
    assert_eq!(appended, "20x5", "text appended");
    let (mut engine, view) = page(s);
    engine
        .execute_script(view, "var e = document.getElementById('s'); e.sheet.disabled = true; e.textContent = '#t{width:30px}'")
        .unwrap();
    assert_eq!(read(&mut engine, view, "document.getElementById('s').sheet.disabled"), "false");
}

#[test]
fn the_disabled_attribute_on_a_style_is_not_the_sheets_flag() {
    // `<style disabled>` is not a thing in HTML: Chromium applies it.
    assert_eq!(size("<style disabled>#t{width:20px}</style>", ""), "20x0");
}

#[test]
fn sheet_text_cannot_close_the_media_attributes_condition_early() {
    // A stray `}` at the top level of a sheet is part of the next rule's
    // prelude, which then fails: Chromium drops that one rule, `media` or
    // not. Wrapping the text as `@media <list> { ... }` let the `}` close
    // the wrapper instead, so the rules after it applied everywhere.
    // Chromium 148.0.7778.216 headless, 800x600 (#964 R1 nit 1).
    let cases = [
        ("screen, stray }", "<style media=\"screen\">a{} } #t{width:20px}</style>", "10x0"),
        ("print, stray }", "<style media=\"print\">a{} } #t{width:20px}</style>", "10x0"),
        ("no media, stray }", "<style>a{} } #t{width:20px}</style>", "10x0"),
        ("screen, stray } then ;", "<style media=\"screen\">a{} }; #t{width:20px}</style>", "10x0"),
        ("screen, only the next rule", "<style media=\"screen\">a{} } b{} #t{width:20px}</style>", "20x0"),
        ("print, only the next rule", "<style media=\"print\">a{} } b{} #t{width:20px}</style>", "10x0"),
        ("screen, at-rule with } ends at ;", "<style media=\"screen\">@layer x }; #t{width:20px}</style>", "20x0"),
        ("screen, } in a function", "<style media=\"screen\">a{b:c(})} #t{width:20px}</style>", "20x0"),
        ("screen, stray } at the end", "<style media=\"screen\">#t{width:20px} }</style>", "20x0"),
        ("print, stray } at the end", "<style media=\"print\">#t{width:20px} }</style>", "10x0"),
    ];
    let mut wrong = Vec::new();
    for (name, body, want) in cases {
        let got = size(body, "");
        if got != want {
            wrong.push(format!("{name}: got {got}, Chromium {want}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `#t`'s size and `#s.sheet.disabled` after `script`.
fn size_and_flag(body: &str, script: &str) -> String {
    let (mut engine, view) = page(body);
    engine.execute_script(view, script).expect("script");
    let w = read(&mut engine, view, "document.getElementById('t').offsetWidth");
    let h = read(&mut engine, view, "document.getElementById('t').offsetHeight");
    let flag = read(&mut engine, view, "document.getElementById('s').sheet.disabled");
    format!("{w}x{h} disabled={flag}")
}

#[test]
fn a_disabled_flag_dies_with_the_sheet_not_with_its_text() {
    // Chromium makes a new, enabled CSSStyleSheet whenever a <style>'s
    // children change or the element is inserted or removed (CSSOM "update
    // a style block"); the flag belongs to that sheet, not to its text.
    // Keying the record by text resurrected a disabled sheet when its text
    // came back (A -> B -> A) and left two same-text sheets entangled.
    // Chromium 148.0.7778.216 headless, 800x600 (#964 R1 HOLD).
    let s = "<style id=s>#t{width:20px}</style>";
    let two = "<style id=s>#t{width:20px}</style><style id=s2>#t{width:20px}</style>";
    let cases = [
        (
            "A -> B -> A by textContent",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; e.textContent='#t{width:30px}'; e.textContent='#t{width:20px}'",
            "20x0 disabled=false",
        ),
        (
            "A -> B -> A with a layout read between",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; e.textContent='#t{width:30px}'; document.body.offsetWidth; e.textContent='#t{width:20px}'",
            "20x0 disabled=false",
        ),
        (
            "A -> A: the same text set again",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; e.textContent=e.textContent",
            "20x0 disabled=false",
        ),
        (
            "A -> B -> A by firstChild.data",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; e.firstChild.data='#t{width:30px}'; e.firstChild.data='#t{width:20px}'",
            "20x0 disabled=false",
        ),
        (
            "A -> B -> A by appendChild then removeChild",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; var n=document.createTextNode(' #t{height:5px}'); e.appendChild(n); e.removeChild(n)",
            "20x0 disabled=false",
        ),
        (
            "A -> B -> A by innerHTML",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; e.innerHTML='#t{width:30px}'; e.innerHTML='#t{width:20px}'",
            "20x0 disabled=false",
        ),
        (
            "removed and reinserted",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; e.remove(); document.head.appendChild(e)",
            "20x0 disabled=false",
        ),
        (
            "removed, reinserted, the new sheet disabled",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; e.remove(); document.head.appendChild(e); e.sheet.disabled=true",
            "10x0 disabled=true",
        ),
        (
            "disabled, new text, the new sheet disabled",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; e.textContent='#t{width:30px}'; e.sheet.disabled=true",
            "10x0 disabled=true",
        ),
        (
            "an old reference cannot enable the new sheet",
            s,
            "var e=document.getElementById('s'); var o=e.sheet; o.disabled=true; e.textContent='#t{width:30px}'; o.disabled=false",
            "30x0 disabled=false",
        ),
        (
            "an old disabled reference cannot disable the new sheet",
            s,
            "var e=document.getElementById('s'); var o=e.sheet; o.disabled=true; e.textContent='#t{width:30px}'; o.disabled=false; o.disabled=true",
            "30x0 disabled=false",
        ),
        ("two nodes, same text: the first disabled", two, "document.getElementById('s').sheet.disabled=true", "20x0 disabled=true"),
        (
            "two nodes, same text: both disabled",
            two,
            "document.getElementById('s').sheet.disabled=true; document.getElementById('s2').sheet.disabled=true",
            "10x0 disabled=true",
        ),
        (
            "a second node with the disabled sheet's text applies",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; var n=document.createElement('style'); n.textContent=e.textContent; document.head.appendChild(n)",
            "20x0 disabled=true",
        ),
        (
            "a media attribute keeps the sheet",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; e.setAttribute('media','screen')",
            "10x0 disabled=true",
        ),
        (
            "a media attribute keeps the sheet, then enabled",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; e.setAttribute('media','screen'); e.sheet.disabled=false",
            "20x0 disabled=false",
        ),
        (
            "insertRule keeps the sheet, then enabled",
            s,
            "var e=document.getElementById('s'); e.sheet.disabled=true; e.sheet.insertRule('#t{height:5px}',1); e.sheet.disabled=false",
            "20x5 disabled=false",
        ),
    ];
    let mut wrong = Vec::new();
    for (name, body, script, want) in cases {
        let got = size_and_flag(body, script);
        if got != want {
            wrong.push(format!("{name}: got {got}, Chromium {want}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_sheet_dies_when_an_ancestor_moves_and_a_stale_wrapper_is_inert() {
    // #964 second HOLD. Chromium 148.0.7778.216 headless, 800x600: a
    // connection change anywhere above a <style> makes a new, enabled
    // sheet (CSSOM "update a style block"), and a CSSStyleSheet object the
    // element no longer owns is inert: its `disabled`, insertRule and
    // deleteRule reach nothing, its ownerNode is null, its rules stay.
    let c = "<div id=c><style id=s>#t{width:20px}</style></div>";
    let cd = "<div id=c><style id=s>#t{width:20px}</style></div><div id=d></div>";
    let cases = [
        (
            "container detached and reinserted, sheet disabled",
            c,
            "var e=document.getElementById('s'); e.sheet.disabled=true; var c=document.getElementById('c'); c.remove(); document.body.appendChild(c)",
            "20x0 disabled=false",
        ),
        (
            "container detached, text replaced, reinserted; the old wrapper disabled",
            c,
            "var e=document.getElementById('s'); var o=e.sheet; var c=document.getElementById('c'); c.remove(); e.textContent='#t{width:30px}'; document.body.appendChild(c); o.disabled=true",
            "30x0 disabled=false",
        ),
        (
            "container detached, text replaced, reinserted; insertRule on the old wrapper",
            c,
            "var e=document.getElementById('s'); var o=e.sheet; var c=document.getElementById('c'); c.remove(); e.textContent='#t{width:30px}'; document.body.appendChild(c); o.insertRule('#t{height:5px}',0)",
            "30x0 disabled=false",
        ),
        (
            "container detached, text replaced, reinserted; insertRule on the new wrapper",
            c,
            "var e=document.getElementById('s'); var c=document.getElementById('c'); c.remove(); e.textContent='#t{width:30px}'; document.body.appendChild(c); e.sheet.insertRule('#t{height:5px}',0)",
            "30x5 disabled=false",
        ),
        (
            "container moved connected to connected, sheet disabled",
            cd,
            "var e=document.getElementById('s'); e.sheet.disabled=true; document.getElementById('d').appendChild(document.getElementById('c'))",
            "20x0 disabled=false",
        ),
        (
            "container moved connected to connected, then the new sheet disabled",
            cd,
            "var e=document.getElementById('s'); e.sheet.disabled=true; document.getElementById('d').appendChild(document.getElementById('c')); e.sheet.disabled=true",
            "10x0 disabled=true",
        ),
        (
            "old enabled wrapper: text changed, then old.disabled = true",
            c,
            "var e=document.getElementById('s'); var o=e.sheet; e.textContent='#t{width:30px}'; o.disabled=true",
            "30x0 disabled=false",
        ),
        (
            "old enabled wrapper: text changed, then old.insertRule",
            c,
            "var e=document.getElementById('s'); var o=e.sheet; e.textContent='#t{width:30px}'; o.insertRule('#t{height:5px}',0)",
            "30x0 disabled=false",
        ),
        (
            "old enabled wrapper: text changed, new sheet read, then old.deleteRule",
            c,
            "var e=document.getElementById('s'); var o=e.sheet; e.textContent='#t{width:30px}'; e.sheet.cssRules.length; o.deleteRule(0)",
            "30x0 disabled=false",
        ),
        (
            "old disabled wrapper: text changed, new sheet read, then old.insertRule",
            c,
            "var e=document.getElementById('s'); var o=e.sheet; o.disabled=true; e.textContent='#t{width:30px}'; e.sheet.cssRules.length; o.insertRule('#t{height:5px}',0)",
            "30x0 disabled=false",
        ),
        (
            "style removed, text replaced, reinserted; the old wrapper disabled",
            c,
            "var e=document.getElementById('s'); var o=e.sheet; e.remove(); e.textContent='#t{width:30px}'; document.body.appendChild(e); o.disabled=true",
            "30x0 disabled=false",
        ),
        (
            "a clone of a disabled style applies, the original stays disabled",
            c,
            "var e=document.getElementById('s'); e.sheet.disabled=true; var k=e.cloneNode(true); k.id='k'; document.body.appendChild(k)",
            "20x0 disabled=true",
        ),
    ];
    let mut wrong = Vec::new();
    for (name, body, script, want) in cases {
        let got = size_and_flag(body, script);
        if got != want {
            wrong.push(format!("{name}: got {got}, Chromium {want}"));
        }
    }
    // replaceChild: the replacement's sheet applies (the old element is gone).
    let replaced = size(
        c,
        "var e=document.getElementById('s'); e.sheet.disabled=true; var k=document.createElement('style'); k.textContent='#t{width:40px}'; e.parentNode.replaceChild(k,e)",
    );
    if replaced != "40x0" {
        wrong.push(format!("replaceChild: got {replaced}, Chromium 40x0"));
    }
    // A wrapper taken before its container moved is a different object from
    // the element's new sheet, owns no node, and keeps its rules.
    let (mut engine, view) = page(c);
    engine
        .execute_script(
            view,
            "var e=document.getElementById('s'); var o=e.sheet; var c=document.getElementById('c'); c.remove(); document.body.appendChild(c);\
             window.__r = [e.sheet===o, o.ownerNode===null, o.cssRules.length, e.sheet.cssRules.length].join(',')",
        )
        .unwrap();
    let identity = read(&mut engine, view, "window.__r");
    if identity != "false,true,1,1" {
        wrong.push(format!("stale wrapper identity: got {identity}, Chromium false,true,1,1"));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
