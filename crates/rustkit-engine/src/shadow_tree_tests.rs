//! Shadow DOM content renders: style, layout and paint walk the flat tree.
//!
//! `tools/parity_oracle/shadow_tree_cases.json` holds each page and what the
//! pinned Chromium reports for it (`shadow_tree_log.mjs --write`):
//! `chrome_boxes`, `id:x:y:width:height` of every element with an id, the
//! document's first and then each shadow tree's (an element that renders no
//! box reads 0:0:0:0); `chrome_hits`, the innermost element under each
//! probe point. The pages size every box in px, so no box depends on font
//! metrics and the test runs on any platform.

use super::*;

/// The expression `shadow_tree_log.mjs` evaluates in Chromium (`BOXES`).
const BOXES: &str = "(function () {\
    var out = [];\
    function walk(root) {\
      Array.prototype.forEach.call(root.querySelectorAll('[id]'), function (e) {\
        var r = e.getBoundingClientRect();\
        out.push(e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height));\
      });\
      Array.prototype.forEach.call(root.querySelectorAll('*'), function (e) { if (e.shadowRoot) walk(e.shadowRoot); });\
    }\
    walk(document);\
    return out.join(' ');\
  })()";

/// Boxes the engine does not place where Chromium does yet, by case. An id
/// listed here that starts to match Chromium fails the test, so the list
/// cannot go stale. Empty since style scoping (slice 2): the acceptance
/// page's shadow `<style>` applies.
const GAPS: &[(&str, &[&str])] = &[];

struct Rendered {
    engine: Engine,
    view: EngineViewId,
}

impl Rendered {
    fn load(html: &str, script: Option<&str>, viewport: (u32, u32)) -> Self {
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds::new(0, 0, viewport.0, viewport.1))
            .expect("headless view");
        // `load_html` runs the page's inline scripts, in order (T8).
        engine.load_html(view, html).expect("load_html");
        let log = engine.script_log(view).expect("view");
        // What a callback threw is the page's (the acceptance page's does,
        // as in Chromium) and is logged at the next lifecycle step.
        let mut scripts = log.iter().filter(|r| r.source.starts_with("inline#"));
        assert!(scripts.all(|r| r.outcome == ScriptOutcome::Ran), "page script: {log:?}");
        if let Some(script) = script {
            engine.execute_script(view, script).expect("case script");
        }
        Rendered { engine, view }
    }

    fn js(&mut self, script: &str) -> String {
        let value = self
            .engine
            .execute_script(self.view, script)
            .expect("script");
        value
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&value)
            .replace("\\\"", "\"")
    }

    /// `id:x:y:width:height` by id.
    fn boxes(&mut self) -> Vec<(String, String)> {
        split_boxes(&self.js(BOXES))
    }

    /// The text runs the display list paints, in order.
    fn painted_text(&self) -> Vec<String> {
        let list = self.engine.views[&self.view]
            .display_list
            .as_ref()
            .expect("display list");
        list.commands
            .iter()
            .filter_map(|cmd| match cmd {
                rustkit_layout::DisplayCommand::Text { text, .. } => Some(text.trim().to_string()),
                _ => None,
            })
            .filter(|t| !t.is_empty())
            .collect()
    }

    /// The innermost element the layout hit test finds at (x, y): its id,
    /// else its tag name.
    fn hit(&self, x: f32, y: f32) -> String {
        let view = &self.engine.views[&self.view];
        let layout = view.layout.as_ref().expect("layout");
        let Some(node) = layout.hit_test(x, y).and_then(|h| h.node_id) else {
            return "none".into();
        };
        let document = view.document.as_ref().expect("document");
        let node = document
            .get_node(rustkit_dom::NodeId::new(node))
            .expect("hit node");
        node.get_attribute("id")
            .map(str::to_string)
            .or_else(|| node.tag_name().map(str::to_string))
            .unwrap_or_else(|| "none".into())
    }
}

fn split_boxes(s: &str) -> Vec<(String, String)> {
    s.split_whitespace()
        .map(|b| {
            let (id, rest) = b.split_once(':').expect("id:x:y:w:h");
            (id.to_string(), rest.to_string())
        })
        .collect()
}

fn cases() -> (serde_json::Value, (u32, u32)) {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/shadow_tree_cases.json"
    ))
    .expect("case file");
    let viewport = (
        data["viewport"][0].as_u64().unwrap() as u32,
        data["viewport"][1].as_u64().unwrap() as u32,
    );
    (data, viewport)
}

fn case<'a>(data: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no case {name}"))
}

fn render(data: &serde_json::Value, name: &str, viewport: (u32, u32)) -> Rendered {
    let c = case(data, name);
    Rendered::load(c["html"].as_str().unwrap(), c["script"].as_str(), viewport)
}

/// Every case's boxes against Chromium's, gaps aside.
#[test]
fn shadow_content_is_laid_out_where_chromium_puts_it() {
    let (data, viewport) = cases();
    let mut wrong = Vec::new();
    let mut gaps_that_pass = Vec::new();
    for c in data["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let chrome = split_boxes(
            c["chrome_boxes"]
                .as_str()
                .expect("run shadow_tree_log.mjs --write"),
        );
        let engine =
            Rendered::load(c["html"].as_str().unwrap(), c["script"].as_str(), viewport).boxes();
        let gaps = GAPS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, g)| *g)
            .unwrap_or(&[]);
        if engine.len() != chrome.len() {
            wrong.push(format!(
                "{name}: engine lists {engine:?}\n   chrome lists {chrome:?}"
            ));
            continue;
        }
        for ((id, want), (engine_id, got)) in chrome.iter().zip(&engine) {
            assert_eq!(id, engine_id, "{name}: the element lists differ in order");
            match (want == got, gaps.contains(&id.as_str())) {
                (false, false) => wrong.push(format!("{name} #{id}: engine {got}, chrome {want}")),
                (true, true) => gaps_that_pass.push(format!("{name} #{id}")),
                _ => {}
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "not where Chromium puts them:\n{}",
        wrong.join("\n")
    );
    assert!(
        gaps_that_pass.is_empty(),
        "listed in GAPS but now match Chromium (remove them): {gaps_that_pass:?}"
    );
}

/// Cases whose hit tests do not match Chromium yet, as `GAPS`. A case
/// listed here that matches fails the test.
const HIT_GAPS: &[&str] = &[];

/// The layout hit test finds the innermost flat-tree element Chromium finds:
/// shadow content, and slotted light nodes (not the slot, which has no box).
#[test]
fn the_hit_test_reaches_into_the_flat_tree() {
    let (data, viewport) = cases();
    let mut wrong = Vec::new();
    for c in data["cases"].as_array().unwrap() {
        let Some(probes) = c["probes"].as_array() else {
            continue;
        };
        let name = c["name"].as_str().unwrap();
        let page = render(&data, name, viewport);
        let chrome: Vec<&str> = c["chrome_hits"].as_str().unwrap().split(' ').collect();
        let engine: Vec<String> = probes
            .iter()
            .map(|p| page.hit(p[0].as_f64().unwrap() as f32, p[1].as_f64().unwrap() as f32))
            .collect();
        match (engine == chrome, HIT_GAPS.contains(&name)) {
            (false, false) => wrong.push(format!("{name}: engine {engine:?}, chrome {chrome:?}")),
            (true, true) => wrong.push(format!("{name} matches Chromium: remove it from HIT_GAPS")),
            _ => {}
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The display list paints the shadow tree and the assigned nodes, and not
/// the light child no slot takes or the fallback of a slot that has nodes.
#[test]
fn the_display_list_paints_the_flat_tree() {
    let (data, viewport) = cases();
    let page = render(&data, "acceptance", viewport);
    assert_eq!(page.painted_text(), ["H", "A", "B", "D", "none"]);
    let page = render(&data, "acceptance-mutated", viewport);
    assert_eq!(page.painted_text(), ["H", "A", "B", "D", "none"]);
    let page = render(&data, "unassigned-child-not-rendered", viewport);
    assert_eq!(page.painted_text(), ["in"]);
    let page = render(&data, "fallback-content", viewport);
    assert_eq!(page.painted_text(), ["fallback", "given"]);
}

/// A page with a shadow host renders the light DOM unchanged around it, and
/// document queries still do not see into the shadow tree.
#[test]
fn document_queries_still_stop_at_the_shadow_boundary() {
    let (data, viewport) = cases();
    let mut page = render(&data, "acceptance", viewport);
    assert_eq!(
        page.js("[String(document.getElementById('hdr')), document.querySelectorAll('slot, header').length, document.getElementById('card').children.length, document.getElementById('card').innerHTML.indexOf('header')].join()"),
        "null,0,4,-1"
    );
    let mut page = render(&data, "declarative-shadow-root", viewport);
    assert_eq!(
        page.js("var h = document.getElementById('host'); [h.shadowRoot.mode, h.shadowRoot.firstChild.id, h.children.length, h.innerHTML, String(document.getElementById('dsd-top')), document.querySelectorAll('template').length].join()"),
        "open,dsd-top,1,<div id=\"dl\" style=\"height:21px\">light</div>,null,1"
    );
}

// ---- Slice 2: style scoping. Each case below is one page of
// shadow_tree_cases.json; the full-file test above checks them all, these
// name the behaviour each one pins.

/// One case's boxes against Chromium's, every id.
fn assert_case_matches_chromium(name: &str) {
    let (data, viewport) = cases();
    let c = case(&data, name);
    let chrome = split_boxes(c["chrome_boxes"].as_str().expect("chrome_boxes"));
    let engine = render(&data, name, viewport).boxes();
    assert_eq!(engine, chrome, "{name}: engine boxes (left) against Chromium's (right)");
}

/// A shadow `<style>` styles its own tree only: the document's `<p>`, and an
/// id the sheet names that exists only in the document, are untouched.
#[test]
fn a_shadow_style_does_not_leak_out() {
    assert_case_matches_chromium("scoped-style-does-not-leak-out");
}

/// Document rules (`.x{display:none}`, `slot{display:block}`, `p{margin:0}`,
/// `div{height}`) do not match inside a shadow tree.
#[test]
fn document_styles_do_not_leak_in() {
    assert_case_matches_chromium("document-style-does-not-leak-in");
}

/// `#ghost` in the document names an id that exists only in a shadow tree:
/// it matches nothing.
#[test]
fn a_document_rule_naming_a_shadow_id_does_not_match() {
    assert_case_matches_chromium("document-rule-naming-a-shadow-id");
}

/// Inherited properties cross the boundary: host -> shadow tree, and the
/// slot's flat-tree parent -> the slotted node (font-size and colour).
#[test]
fn inherited_properties_cross_into_the_shadow_tree() {
    assert_case_matches_chromium("inheritance-into-the-shadow-tree");
}

/// `:host` and `:host(.wide)` style the host from inside; an outer rule on
/// the host beats either whatever its specificity; `:host(.wide) .lab`,
/// `:host .lab` and `:host > .lab` match inside; `div .lab` does not reach
/// past the shadow root.
#[test]
fn host_rules_with_and_without_an_argument() {
    assert_case_matches_chromium("host-rules");
}

/// `::slotted(p)` styles assigned nodes only (not a `<p>` inside one), and a
/// light-DOM rule beats it whatever their specificities.
#[test]
fn slotted_rules_lose_to_light_rules() {
    assert_case_matches_chromium("slotted-rules");
}

/// Two nested shadow trees, each with its own `<style>`: each sheet styles
/// its own tree, and the inner `:host` styles the inner host.
#[test]
fn nested_shadow_trees_each_use_their_own_styles() {
    assert_case_matches_chromium("nested-shadow-styles");
}

/// `:defined` / `:not(:defined)`: a custom element is defined once
/// `customElements.define` ran for its name; one never defined is not;
/// every other element is.
#[test]
fn defined_follows_the_custom_element_registry() {
    assert_case_matches_chromium("defined-pseudo-class");
}

/// The text colour and background of every element with a box, in the
/// cases that set `colors`, against Chromium's `getComputedStyle`.
#[test]
fn scoped_colours_match_chromium() {
    let (data, viewport) = cases();
    let mut wrong = Vec::new();
    for c in data["cases"].as_array().unwrap() {
        let Some(chrome) = c["chrome_colors"].as_str() else {
            continue;
        };
        let name = c["name"].as_str().unwrap();
        let page = render(&data, name, viewport);
        for entry in chrome.split(' ') {
            let (id, want) = entry.split_once(':').unwrap();
            if want == "-" {
                continue;
            }
            let got = page.colors_of(id);
            if got != want {
                wrong.push(format!("{name} #{id}: engine {got}, chrome {want}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A `<style>` added to a shadow root after the first layout restyles that
/// tree (and only it); removing it again restores the earlier style.
#[test]
fn adding_and_removing_a_shadow_style_after_layout_restyles() {
    let html = "<!doctype html><html><head><style>body{margin:0;font:16px/20px monospace}</style></head><body><div id=h></div><div id=light>l</div><script>document.getElementById('h').attachShadow({mode:'open'}).innerHTML = '<div id=t>x</div>';</script></body></html>";
    let mut page = Rendered::load(html, None, (800, 600));
    let boxes = |page: &mut Rendered| {
        page.boxes()
            .into_iter()
            .map(|(id, b)| format!("{id}:{b}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert_eq!(boxes(&mut page), "h:0:0:800:20 light:0:20:800:20 t:0:0:800:20");
    page.js("var s = document.createElement('style'); s.id = 'sheet'; s.textContent = 'div{height:12px}'; document.getElementById('h').shadowRoot.appendChild(s); 'ok'");
    assert_eq!(boxes(&mut page), "h:0:0:800:12 light:0:12:800:20 t:0:0:800:12 sheet:0:0:0:0");
    page.js("document.getElementById('h').shadowRoot.getElementById('sheet').textContent = 'div{height:14px}'; 'ok'");
    assert_eq!(boxes(&mut page), "h:0:0:800:14 light:0:14:800:20 t:0:0:800:14 sheet:0:0:0:0");
    page.js("document.getElementById('h').shadowRoot.getElementById('sheet').remove(); 'ok'");
    assert_eq!(boxes(&mut page), "h:0:0:800:20 light:0:20:800:20 t:0:0:800:20");
}

impl Rendered {
    /// `color/background` of the element with this id (shadow trees
    /// included) as Chromium serializes them, from the box the engine laid
    /// out for it; `-` when it has none.
    fn colors_of(&self, id: &str) -> String {
        let view = &self.engine.views[&self.view];
        let document = view.document.as_ref().expect("document");
        let mut node = None;
        document.traverse_shadow_including(|n| {
            if node.is_none() && n.get_attribute("id") == Some(id) {
                node = Some(n.id.raw());
            }
        });
        let Some(node) = node else {
            return "no element".into();
        };
        fn find(b: &rustkit_layout::LayoutBox, node: usize) -> Option<&rustkit_layout::LayoutBox> {
            if b.node_id == Some(node) {
                return Some(b);
            }
            b.children.iter().find_map(|c| find(c, node))
        }
        let Some(b) = find(view.layout.as_ref().expect("layout"), node) else {
            return "-".into();
        };
        fn css(c: &rustkit_css::Color) -> String {
            if c.a >= 1.0 {
                format!("rgb({},{},{})", c.r, c.g, c.b)
            } else {
                format!("rgba({},{},{},{})", c.r, c.g, c.b, c.a)
            }
        }
        format!("{}/{}", css(&b.style.color), css(&b.style.background_color))
    }
}

/// Whether each element with an id is painted, against Chromium's
/// `chrome_painted`, in the cases that set `painted` (the component page:
/// custom elements defined with and without shadow roots, never defined,
/// declarative roots with and without slots, `:host` opacity and
/// visibility, `::slotted` visibility, `:not(:defined)` rules). An element
/// is painted when the display list fills its background colour; each
/// element of such a case has its own.
#[test]
fn the_display_list_paints_what_chromium_paints() {
    let (data, viewport) = cases();
    let mut wrong = Vec::new();
    for c in data["cases"].as_array().unwrap() {
        let Some(chrome) = c["chrome_painted"].as_str() else {
            continue;
        };
        let name = c["name"].as_str().unwrap();
        let page = render(&data, name, viewport);
        wrong.extend(page.painted_against(name, chrome));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---- A definition that never arrives (macOS receipt, microsoft.com).

/// A header custom element with light children, as on microsoft.com: a
/// skip link, a banner element and a `header` with a logo. Its sheet hides
/// it until it is defined; the page's script defines it. Chromium runs the
/// script (after a first one that runs for 400 ms) and lays it out at
/// `wrap:0:0:800:55 skip:0:0:1:1 hd:0:1:800:54 logo:79:17:108:23
/// main:0:55:800:100`, all painted (Chromium 141).
const UNDEFINED_HEADER: &str = "<!doctype html><html><head><style>body{margin:0;font:16px/20px monospace} \
    x-hdr:not(:defined){display:block;height:54px;visibility:hidden} x-hdr{display:block}</style></head><body>\
    <div id=wrap style=\"background:rgb(1,2,3)\"><x-hdr id=hdr>\
    <a id=skip href=#main style=\"display:block;height:1px;width:1px;background:rgb(10,20,30)\"></a>\
    <x-banner id=ban></x-banner>\
    <header id=hd style=\"height:54px;padding-top:16px;box-sizing:border-box;background:rgb(40,50,60)\">\
    <div id=logo style=\"width:108px;height:23px;margin-left:79px;background:rgb(70,80,90)\"></div></header>\
    </x-hdr></div><main id=main style=\"height:100px;background:rgb(100,110,120)\"></main>\
    <script>var t = Date.now(); while (Date.now() - t < 400) {}</script>\
    <script>customElements.define('x-hdr', class extends HTMLElement {});</script></body></html>";

/// Loads `html` through the navigation path (page scripts, load event),
/// with `budget_ms` for the page's scripts.
fn navigate(html: &str, budget_ms: u64) -> Rendered {
    navigate_with(html, vec![], budget_ms)
}

/// [`navigate`], with `routes` served beside the page. Any other path is
/// served as text, except `/missing...`, which is a 404.
fn navigate_with(
    html: &str,
    mut routes: Vec<(&'static str, &'static str, String)>,
    budget_ms: u64,
) -> Rendered {
    routes.push(("/", "text/html", html.to_string()));
    let server = crate::script_net_tests::serve_routes(routes);
    let config = EngineConfig {
        script_budget_ms: budget_ms,
        ..EngineConfig::default()
    };
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds::new(0, 0, 800, 600))
        .expect("headless view");
    let url = url::Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    Rendered { engine, view }
}

impl Rendered {
    /// The solid colours the display list fills, as `r,g,b`.
    fn painted_colours(&self) -> Vec<String> {
        let list = self.engine.views[&self.view]
            .display_list
            .as_ref()
            .expect("display list");
        list.commands
            .iter()
            .filter_map(|cmd| match cmd {
                rustkit_layout::DisplayCommand::SolidColor(c, _) => {
                    Some(format!("{},{},{}", c.r, c.g, c.b))
                }
                _ => None,
            })
            .collect()
    }

    /// Each `id:P` or `id:-` of Chromium's `chrome_painted` that the
    /// display list does not agree with, as `case #id: engine .., chrome ..`.
    fn painted_against(&self, name: &str, chrome: &str) -> Vec<String> {
        let painted = self.painted_colours();
        let mut wrong = Vec::new();
        for entry in chrome.split(' ') {
            let (id, want) = entry.split_once(':').unwrap();
            let background = self.colors_of(id);
            let got = match background.split_once('/') {
                Some((_, bg)) => {
                    let rgb = bg
                        .trim_start_matches("rgba(")
                        .trim_start_matches("rgb(")
                        .trim_end_matches(')');
                    let rgb: Vec<&str> = rgb.split(',').take(3).collect();
                    match painted.contains(&rgb.join(",")) {
                        true => "P",
                        false => "-",
                    }
                }
                None => "-",
            };
            if got != want {
                wrong.push(format!(
                    "{name} #{id}: engine {got}, chrome {want} ({background})"
                ));
            }
        }
        wrong
    }
}

/// The page's defining script did not run in the engine (here: the first
/// script spent the page's 200 ms script budget; on a live page also a
/// script that fails or is skipped), so the header's
/// definition never arrives. Once the page's scripts are over, its
/// `:not(:defined)` rule must not keep the header's children unpainted:
/// they are laid out where Chromium puts them, and painted.
#[test]
fn light_children_of_an_element_whose_definition_never_arrives_are_painted() {
    let mut page = navigate(UNDEFINED_HEADER, 200);
    assert_eq!(
        page.js("String(customElements.get('x-hdr'))"),
        "undefined",
        "the page's script was meant not to run"
    );
    let boxes: Vec<String> = page
        .boxes()
        .into_iter()
        .filter(|(id, _)| id != "hdr" && id != "ban")
        .map(|(id, b)| format!("{id}:{b}"))
        .collect();
    let painted = page.painted_colours();
    let unpainted: Vec<&str> = [
        ("wrap", "1,2,3"),
        ("skip", "10,20,30"),
        ("hd", "40,50,60"),
        ("logo", "70,80,90"),
        ("main", "100,110,120"),
    ]
    .into_iter()
    .filter(|(_, colour)| !painted.iter().any(|c| c == colour))
    .map(|(id, _)| id)
    .collect();
    assert!(
        unpainted.is_empty() && boxes.join(" ") == "wrap:0:0:800:55 skip:0:0:1:1 hd:0:1:800:54 logo:79:17:108:23 main:0:55:800:100",
        "laid out at {boxes:?}, against Chromium's wrap:0:0:800:55 skip:0:0:1:1 hd:0:1:800:54 logo:79:17:108:23 main:0:55:800:100;\n\
         not painted: {unpainted:?}"
    );
}

// ---- The settle is narrow (R1 HOLD @ 3f1ff2b7): it changes the cascade
//      only, only after a load that lost a script, and only for the names
//      present and undefined when the load's scripts were over.

/// The reviewer's page: a custom element no script ever defines, hidden by
/// its own `:not(:defined)` rule, and a script that waits for it. With
/// `lost`, a script before it throws at top level: the load lost a script.
/// `x-new` is a name that first appears after the load.
fn never_defined_page(lost: bool) -> String {
    format!(
        "<!doctype html><style>body{{margin:0}} x-never:not(:defined){{visibility:hidden}} \
         x-new:not(:defined){{visibility:hidden}}</style>\
         <x-never id=n><span style='display:block;width:10px;height:10px;background:rgb(1,1,1)'></span></x-never>\
         {}<script>var wd; customElements.whenDefined('x-never').then(function(){{wd=1}})</script>",
        if lost { "<script>throw new Error('lost')</script>" } else { "" }
    )
}

/// What the reviewer's script reads after the load. Chromium 141 (and this
/// branch at `a43800ed`): `false,undefined,undefined,1`.
const NEVER_DEFINED_PROBE: &str = "[document.getElementById('n').matches(':defined'), \
    String(customElements.get('x-never')), String(wd), \
    document.querySelectorAll(':not(:defined)').length].join()";

/// Did the engine lose one of the load's `<script>` elements (the settle's
/// own test, `load_lost_a_script`)?
fn lost_a_script(page: &Rendered) -> bool {
    load_lost_a_script(page.engine.script_log(page.view).unwrap())
}

/// Script-facing matching follows the registry, whether or not the load
/// lost a script: `matches`, `querySelectorAll` and `closest` see the
/// never-defined element as `:not(:defined)`, and so does a new element of
/// another name, as in Chromium.
#[test]
fn script_sees_a_never_defined_element_as_not_defined_after_the_load() {
    for lost in [true, false] {
        let mut page = navigate(&never_defined_page(lost), 5_000);
        assert_eq!(lost_a_script(&page), lost, "lost={lost}");
        assert_eq!(
            page.js(NEVER_DEFINED_PROBE),
            "false,undefined,undefined,1",
            "lost={lost}"
        );
        assert_eq!(
            page.js(
                "[document.querySelector('span').closest(':not(:defined)').id, \
                  document.body.appendChild(document.createElement('x-new')).matches(':defined')].join()"
            ),
            "n,false",
            "lost={lost}"
        );
    }
}

/// Every script ran and the element is never defined: Chromium keeps its
/// `:not(:defined){visibility:hidden}` content unpainted, and so do we.
#[test]
fn a_load_whose_scripts_all_ran_keeps_not_defined_content_hidden() {
    let page = navigate(&never_defined_page(false), 5_000);
    assert!(!lost_a_script(&page));
    assert!(
        !page.painted_colours().iter().any(|c| c == "1,1,1"),
        "x-never's child is painted"
    );
}

/// After a load that lost a script, the never-defined element present then
/// is `:defined` for style (its child is painted). An element of a name
/// that first appears later is not, for style or for script.
#[test]
fn a_name_that_first_appears_after_the_settle_follows_the_registry() {
    let mut page = navigate(&never_defined_page(true), 5_000);
    assert!(lost_a_script(&page));
    assert!(
        page.painted_colours().iter().any(|c| c == "1,1,1"),
        "the settle did not reach x-never's child"
    );
    assert_eq!(
        page.js(
            "var nw = document.createElement('x-new'); nw.id = 'nw'; \
             nw.innerHTML = \"<span style='display:block;width:10px;height:10px;background:rgb(2,2,2)'></span>\"; \
             document.body.appendChild(nw); String(nw.matches(':defined'))"
        ),
        "false"
    );
    // The append was laid out (`execute_script` flushes); ask again after it.
    assert_eq!(
        page.js("String(document.getElementById('nw').matches(':defined'))"),
        "false"
    );
    assert!(
        !page.painted_colours().iter().any(|c| c == "2,2,2"),
        "x-new's child is painted: a later name was settled"
    );
    assert!(page.painted_colours().iter().any(|c| c == "1,1,1"));
}

/// A settled name whose real definition arrives after the load (a timer
/// past the timer horizon): the upgrade runs once, its shadow content is
/// painted, and the light child no slot takes is no longer painted.
#[test]
fn a_late_definition_of_a_settled_name_upgrades_once() {
    let html = "<!doctype html><style>body{margin:0} x-late{display:block} \
        x-late:not(:defined){visibility:hidden}</style>\
        <x-late id=l><div style='width:10px;height:10px;background:rgb(3,3,3)'></div></x-late>\
        <script>throw new Error('lost')</script>\
        <script>var made = 0, connected = 0; setTimeout(function () { \
          customElements.define('x-late', class extends HTMLElement { \
            constructor() { super(); made++; this.attachShadow({mode: 'open'}).innerHTML = \
              \"<div style='width:10px;height:10px;background:rgb(4,4,4)'></div>\"; } \
            connectedCallback() { connected++; } }); }, 8000)</script>";
    let mut page = navigate(html, 5_000);
    assert!(lost_a_script(&page));
    assert!(
        page.painted_colours().iter().any(|c| c == "3,3,3"),
        "settled: the light child shows until the definition arrives"
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let pumped = rt.block_on(page.engine.pump_live(page.view, 9_000));
    assert_eq!(pumped.timers_ran, 1);
    assert_eq!(
        page.js("[made, connected, document.getElementById('l').matches(':defined')].join()"),
        "1,1,true"
    );
    let painted = page.painted_colours();
    assert!(
        painted.iter().any(|c| c == "4,4,4"),
        "shadow content not painted"
    );
    assert!(
        !painted.iter().any(|c| c == "3,3,3"),
        "the unslotted light child is still painted"
    );
}

/// The component page, loaded the way the browser loads it (`load_url`),
/// with every script running: each element painted or not as Chromium's
/// committed `chrome_painted` says (`c-hold` stays hidden by its own
/// `:not(:defined)` rule).
#[test]
fn the_component_page_on_the_navigation_path_paints_what_chromium_paints() {
    let (data, _) = cases();
    let c = case(&data, "component-page");
    let page = navigate(c["html"].as_str().unwrap(), 5_000);
    assert!(
        !lost_a_script(&page),
        "{:#?}",
        page.engine.script_log(page.view)
    );
    let wrong = page.painted_against("component-page", c["chrome_painted"].as_str().unwrap());
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---- Only a script the engine lost settles (R1 HOLD @ 8babcc15). A
//      `nomodule` script that a module-capable browser skips, and a script
//      whose server answered with an error status, are lost in Chromium
//      too: the page behaves the same there, so they do not settle.

/// The reviewer's page: a menu whose component is defined on first click,
/// hidden by its own `:not(:defined){display:none}` rule until then. `rest`
/// is the page's other script, the one under test.
fn lazy_menu_page(rest: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0}} x-lazy:not(:defined){{display:none}}</style>\
         <x-lazy id=m><div id=mi style=\"height:40px\">secret menu</div></x-lazy>\
         <script type=module>document.addEventListener('click',function(){{\
           customElements.define('x-lazy',class extends HTMLElement{{}})}})</script>\
         {rest}"
    )
}

/// What the reviewer reads after the load: the menu item's height and the
/// menu's display. Chromium 141 keeps the menu hidden: `0,none`.
const LAZY_MENU_PROBE: &str =
    "[Math.round(document.getElementById('mi').getBoundingClientRect().height), \
    getComputedStyle(document.getElementById('m')).display].join()";

/// Loads [`lazy_menu_page`] with `rest`, and reads [`LAZY_MENU_PROBE`].
fn lazy_menu(rest: &str, budget_ms: u64) -> String {
    let routes = vec![(
        "/legacy.js",
        "text/javascript",
        "window.legacy = 1;".to_string(),
    )];
    let mut page = navigate_with(&lazy_menu_page(rest), routes, budget_ms);
    assert_eq!(
        page.js("String(customElements.get('x-lazy'))"),
        "undefined",
        "{:#?}",
        page.engine.script_log(page.view)
    );
    page.js(LAZY_MENU_PROBE)
}

/// A module/nomodule pair whose module ran: Chromium skips the `nomodule`
/// script by design and keeps the menu hidden; so do we. External and
/// inline `nomodule` alike.
#[test]
fn a_nomodule_script_beside_a_module_does_not_settle() {
    for rest in [
        "<script nomodule src=/legacy.js></script>",
        "<script nomodule>window.legacy = 1</script>",
    ] {
        assert_eq!(lazy_menu(rest, 5_000), "0,none", "{rest}");
    }
}

/// The same page with a script that really is lost (a top-level `throw`)
/// still settles: the menu is laid out.
#[test]
fn the_lazy_menu_with_a_lost_script_settles() {
    let got = lazy_menu(
        "<script nomodule src=/legacy.js></script><script>throw new Error('lost')</script>",
        5_000,
    );
    assert!(got.starts_with("40,") && !got.ends_with(",none"), "{got}");
}

/// A script whose server answered 404: Chromium loses it too, and keeps
/// the menu hidden; so do we.
#[test]
fn a_script_answered_with_an_error_status_does_not_settle() {
    assert_eq!(
        lazy_menu("<script src=/missing-analytics.js></script>", 5_000),
        "0,none"
    );
}

/// A script lost on the engine's side still settles: one whose fetch got
/// no HTTP response at all, and one the script budget never started (the
/// first script spends it). The first is refused by the engine's address
/// policy before any connection (a loopback `src` on another port than
/// the page's: `Address not permitted`), so it never reaches the socket;
/// no test here reaches a refused connection.
#[test]
fn a_script_lost_on_the_engines_side_settles() {
    for (rest, budget_ms) in [
        (
            "<script src=\"http://127.0.0.1:1/gone.js\"></script>",
            5_000,
        ),
        (
            "<script>var t = Date.now(); while (Date.now() - t < 400) {}</script>\
             <script>window.late = 1</script>",
            200,
        ),
    ] {
        let got = lazy_menu(rest, budget_ms);
        assert!(
            got.starts_with("40,") && !got.ends_with(",none"),
            "{rest}: {got}"
        );
    }
}

/// Which outcomes are a lost script. Chromium loses a `nomodule` script
/// and one answered with any error status too; anything the engine failed
/// on its own side is lost.
#[test]
fn a_lost_script_is_one_chromium_would_have_run() {
    let record = |source: &str, outcome: ScriptOutcome| ScriptRecord {
        source: source.into(),
        bytes: 0,
        elapsed_ms: 0,
        outcome,
    };
    for outcome in [
        ScriptOutcome::Ran,
        ScriptOutcome::Nomodule,
        ScriptOutcome::FetchFailed(FetchFailure::Status(404)),
        ScriptOutcome::FetchFailed(FetchFailure::Status(410)),
        ScriptOutcome::FetchFailed(FetchFailure::Status(503)),
    ] {
        assert!(
            !load_lost_a_script(&[record("http://a/x.js", outcome.clone())]),
            "{outcome:?}"
        );
    }
    for outcome in [
        ScriptOutcome::OverBudget,
        ScriptOutcome::Threw("Error: lost".into()),
        ScriptOutcome::Skipped("too large to run inside the script budget"),
        ScriptOutcome::FetchFailed(FetchFailure::Transport("connection refused".into())),
    ] {
        assert!(
            load_lost_a_script(&[record("http://a/x.js", outcome.clone())]),
            "{outcome:?}"
        );
    }
    // An error that escaped a callback is not a lost script.
    assert!(!load_lost_a_script(&[record(
        "timers",
        ScriptOutcome::Threw("x".into())
    )]));
    assert_eq!(FetchFailure::Status(404).to_string(), "HTTP 404 Not Found");
}
