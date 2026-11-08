//! LAT4: a relayout takes the document's prepared sheets (the parsed
//! sheets kept for the viewport, ranked by `@layer`, and the custom
//! properties) from the view instead of copying, filtering and scanning
//! every rule again, unless a sheet changed. A `<style>` text change
//! (CSSOM `insertRule` writes the text) re-parses that sheet only; an
//! adopted sheet's change, a late `<link>` sheet or a new viewport
//! prepares the set again.

use super::*;

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

/// `sheets` `<style>` sheets of `rules` rules each (every tenth inside an
/// `@media`), `vars` custom properties on `:root`, and a `#t` box the
/// tests size. Sheet `s0` holds `#t`'s own rule.
fn page(sheets: usize, rules: usize, vars: usize, extra_head: &str) -> String {
    let mut html = String::from("<html><head>");
    html.push_str("<style id=s0>body{margin:0} #t{width:40px;height:10px} :root{");
    for v in 0..vars {
        html.push_str(&format!("--v{v}:#{:06x};", v * 97 % 0xffffff));
    }
    html.push_str("}</style>");
    for s in 1..sheets {
        html.push_str(&format!("<style id=s{s}>"));
        for r in 0..rules {
            let rule = format!(
                ".c{s}-{r} .d{r} > span:hover, .c{s}-{r}::before {{ margin-left: {}px; color: var(--v{}) }}",
                r % 7,
                r % vars.max(1)
            );
            match r % 10 {
                0 => html.push_str(&format!("@media (min-width: {}px) {{ {rule} }}", 400 + r)),
                _ => html.push_str(&rule),
            }
        }
        html.push_str("</style>");
    }
    html.push_str(extra_head);
    html.push_str("</head><body><div id=t></div>");
    for i in 0..200 {
        html.push_str(&format!("<div class='c1-{} d{}'><span>row {i}</span></div>", i % rules.max(1), i % 13));
    }
    html.push_str("</body></html>");
    html
}

fn load(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, html).expect("load_html");
    (engine, view)
}

/// What the builds of `f` did with their sheets.
fn extracts(f: impl FnOnce()) -> SheetExtract {
    let before = SheetExtract::totals();
    f();
    SheetExtract::totals().since(before)
}

#[test]
fn a_second_full_relayout_with_no_sheet_change_parses_and_prepares_nothing() {
    let (mut engine, view) = load(&page(20, 50, 40, ""));
    engine.relayout(view).expect("relayout");
    let second = extracts(|| engine.relayout(view).expect("relayout"));
    assert_eq!(second.builds, 1);
    assert_eq!(second.parsed, 0, "{second:?}");
    assert_eq!(second.reused, 20, "{second:?}");
    assert_eq!((second.prepared_built, second.prepared_reused), (0, 1), "{second:?}");
}

#[test]
fn insert_rule_reparses_only_that_sheet_and_the_rule_applies() {
    let (mut engine, view) = load(&page(20, 50, 40, ""));
    engine.relayout(view).expect("relayout");
    assert_eq!(read(&mut engine, view, "document.getElementById('t').offsetWidth"), "40");
    let edit = extracts(|| {
        engine
            .execute_script(view, "var s = document.getElementById('s7').sheet; s.insertRule('#t { width: 77px }', s.cssRules.length)")
            .expect("insertRule");
        engine.relayout(view).expect("relayout");
    });
    assert_eq!(edit.parsed, 1, "{edit:?}");
    assert_eq!(edit.prepared_built, 1, "{edit:?}");
    assert_eq!(read(&mut engine, view, "document.getElementById('t').offsetWidth"), "77");
    let after = extracts(|| engine.relayout(view).expect("relayout"));
    assert_eq!((after.parsed, after.prepared_built), (0, 0), "{after:?}");
    assert_eq!(read(&mut engine, view, "document.getElementById('t').offsetWidth"), "77");
}

#[test]
fn a_style_text_mutation_reparses_only_that_sheet() {
    let (mut engine, view) = load(&page(20, 50, 40, ""));
    engine.relayout(view).expect("relayout");
    let edit = extracts(|| {
        engine
            .execute_script(view, "document.getElementById('s3').textContent = '#t { height: 33px }'")
            .expect("textContent");
        engine.relayout(view).expect("relayout");
    });
    assert_eq!(edit.parsed, 1, "{edit:?}");
    assert_eq!(edit.prepared_built, 1, "{edit:?}");
    assert_eq!(read(&mut engine, view, "document.getElementById('t').offsetHeight"), "33");
    // Removing a `<style>` takes its rules out.
    engine.execute_script(view, "document.getElementById('s3').remove()").expect("remove");
    engine.relayout(view).expect("relayout");
    assert_eq!(read(&mut engine, view, "document.getElementById('t').offsetHeight"), "10");
}

#[test]
fn a_disabled_sheet_stops_applying_and_applies_again_when_enabled() {
    let (mut engine, view) = load(&page(5, 20, 10, ""));
    engine
        .execute_script(
            view,
            "window.__s = new CSSStyleSheet(); __s.replaceSync('#t { width: 55px }'); document.adoptedStyleSheets = [__s];",
        )
        .expect("adopt");
    engine.relayout(view).expect("relayout");
    engine.relayout(view).expect("relayout");
    let width = |engine: &mut Engine| read(engine, view, "document.getElementById('t').offsetWidth");
    assert_eq!(width(&mut engine), "55");
    let off = extracts(|| {
        engine.execute_script(view, "__s.disabled = true").expect("disable");
        engine.relayout(view).expect("relayout");
    });
    assert_eq!(off.prepared_built, 1, "{off:?}");
    assert_eq!(width(&mut engine), "40");
    engine.execute_script(view, "__s.disabled = false").expect("enable");
    engine.relayout(view).expect("relayout");
    assert_eq!(width(&mut engine), "55");
    let again = extracts(|| engine.relayout(view).expect("relayout"));
    assert_eq!((again.parsed, again.prepared_reused), (0, 1), "{again:?}");
}

#[test]
fn a_late_external_sheet_applies_when_it_arrives() {
    let (mut engine, view) = load(&page(5, 20, 10, ""));
    engine.relayout(view).expect("relayout");
    engine.relayout(view).expect("relayout");
    // As the live turn adds a sheet that missed the load's budget.
    let late = Stylesheet::parse("#t { width: 66px }").expect("css");
    Rc::make_mut(&mut engine.views.get_mut(&view).unwrap().external_stylesheets).push(late);
    let arrived = extracts(|| engine.relayout(view).expect("relayout"));
    assert_eq!(arrived.prepared_built, 1, "{arrived:?}");
    assert_eq!(read(&mut engine, view, "document.getElementById('t').offsetWidth"), "66");
    // And a replaced list (a new document's sheets) likewise.
    engine.views.get_mut(&view).unwrap().external_stylesheets =
        Rc::new(vec![Stylesheet::parse("#t { width: 67px }").expect("css")]);
    engine.relayout(view).expect("relayout");
    assert_eq!(read(&mut engine, view, "document.getElementById('t').offsetWidth"), "67");
}

#[test]
fn custom_properties_stay_correct_after_a_sheet_change() {
    let head = "<style id=vars>:root { --w: 40px }</style><style>#t { width: var(--w) }</style>";
    let (mut engine, view) = load(&page(5, 20, 10, head));
    engine.relayout(view).expect("relayout");
    engine.relayout(view).expect("relayout");
    let width = |engine: &mut Engine| read(engine, view, "document.getElementById('t').offsetWidth");
    assert_eq!(width(&mut engine), "40");
    engine
        .execute_script(view, "document.getElementById('vars').textContent = ':root { --w: 60px }'")
        .expect("textContent");
    engine.relayout(view).expect("relayout");
    assert_eq!(width(&mut engine), "60");
    let again = extracts(|| engine.relayout(view).expect("relayout"));
    assert_eq!(again.prepared_reused, 1, "{again:?}");
    assert_eq!(width(&mut engine), "60");
}

#[test]
fn media_rules_follow_the_viewport_over_cached_sheets() {
    let head = "<style>@media (min-width: 1000px) { #t { height: 99px } }</style>";
    let (mut engine, view) = load(&page(5, 20, 10, head));
    engine.relayout(view).expect("relayout");
    engine.relayout(view).expect("relayout");
    let height = |engine: &mut Engine| read(engine, view, "document.getElementById('t').offsetHeight");
    assert_eq!(height(&mut engine), "10");
    let wide = extracts(|| {
        engine
            .resize_view(view, Bounds { x: 0, y: 0, width: 1200, height: 600 })
            .expect("resize");
    });
    assert_eq!(wide.parsed, 0, "{wide:?}");
    assert_eq!(height(&mut engine), "99");
    engine
        .resize_view(view, Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("resize");
    assert_eq!(height(&mut engine), "10");
}

/// The LAT4 measurement: a page of 20 sheets x 250 rules (5,000) and 300
/// custom properties, relaid out with no change. Prints each relayout's
/// `build_ms` and the extraction share of it. Not a gate (timing).
#[test]
#[ignore]
fn measure_build_ms_on_a_5k_rule_page() {
    let (mut engine, view) = load(&page(20, 250, 300, ""));
    for i in 0..6 {
        let started = std::time::Instant::now();
        let one = extracts(|| engine.relayout(view).expect("relayout"));
        let took = started.elapsed().as_secs_f64() * 1000.0;
        let build = engine.views[&view].relayout_stats.last_build_ms;
        eprintln!("LAT4 relayout {i}: took_ms={took:.1} build_ms={build:.1} {one:?}");
    }
}

/// Where the rest of a relayout goes on a page shaped like the logs'
/// slowest one: the 5k-rule sheets plus 150 icons drawn from a sprite
/// sheet by `<use href=#…>` and a few svg paint rules. `took_ms` minus
/// the stages is the work before the build (svg paint styles, inline svg
/// cache). Not a gate (timing).
#[test]
#[ignore]
fn measure_relayout_on_a_sprite_icon_page() {
    let mut sprite = String::from("<svg style='display:none'>");
    for k in 0..40 {
        sprite.push_str(&format!("<symbol id=ico{k} viewBox='0 0 10 10'><path d='M0 0L{k} 10Z'/></symbol>"));
    }
    sprite.push_str("</svg>");
    let mut icons = String::new();
    for i in 0..150 {
        icons.push_str(&format!("<svg class=ic width=10 height=10><use href='#ico{}'/></svg>", i % 40));
    }
    let head = "<style>.ic path, .ic use { fill: var(--v1) } svg.ic { stroke: currentColor }</style>";
    let html = page(20, 250, 300, head).replace("<body><div id=t></div>", &format!("<body>{sprite}<div id=t></div>{icons}"));
    let (mut engine, view) = load(&html);
    for i in 0..4 {
        let started = std::time::Instant::now();
        relayout_cause::take_stage_ms();
        let one = extracts(|| engine.relayout(view).expect("relayout"));
        let took = started.elapsed().as_secs_f64() * 1000.0;
        let build = engine.views[&view].relayout_stats.last_build_ms;
        eprintln!("LAT4 sprite relayout {i}: took_ms={took:.1} build_ms={build:.1} {one:?}");
    }
}
