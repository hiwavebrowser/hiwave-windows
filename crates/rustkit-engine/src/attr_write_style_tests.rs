//! Every attribute writer reaches the style the page computes: after each write the computed `color` is what the
//! rule matching the new `id`, `class` or `data-a` gives. The write path skips the steps that only act on `on<type>`,
//! `async`, `src` and `srcset` for every other name (rustkit-bindings dom.rs); this is the page-level check that
//! the style side still sees each write. The sequence and its answers are Chrome for Testing 148.0.7778.216's
//! (`/tmp/js1/aw/scen.js`, part 1).

use super::*;

const PAGE: &str = r#"<!doctype html><html><head><style>
    .k { color: rgb(4, 5, 6) } [data-a=on] { color: rgb(7, 8, 9) } #x { color: rgb(1, 2, 3) }
    </style></head><body><div id="e"></div><x-q id="q"></x-q></body></html>"#;

fn run(define_custom_element: bool) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 1280, 800))
        .expect("headless view");
    engine.load_html(id, PAGE).expect("load_html");
    // With a definition the attribute-change reaction is installed on the same write path.
    let define = if define_custom_element {
        "customElements.define('x-q', class extends HTMLElement { static get observedAttributes() { return ['class', 'id', 'data-a']; } attributeChangedCallback() {} });"
    } else {
        ""
    };
    let script = format!(
        "{define} var e = document.getElementById('e');
         function c() {{ return getComputedStyle(e).color.replace(/rgb\\(|\\)| /g, ''); }}
         var s = [c()];
         e.className = 'k'; s.push(c());
         e.setAttribute('class', ''); s.push(c());
         e.setAttribute('CLASS', 'k'); s.push(c());
         e.removeAttribute('Class'); s.push(c());
         e.setAttribute('data-a', 'on'); s.push(c());
         e.setAttribute('DATA-A', 'off'); s.push(c());
         e.setAttribute('data-a', 'on'); e.toggleAttribute('data-a'); s.push(c());
         e.id = 'x'; s.push(c());
         e.removeAttribute('id'); s.push(c());
         e.setAttribute('ID', 'x'); s.push(c());
         e.classList.add('k'); s.push(c());
         e.id = ''; s.push(c());
         s.join(';')"
    );
    let out = engine.execute_script(id, &format!("String(eval({script:?}))")).expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .unwrap_or_else(|| panic!("evaluated to {out}"))
        .to_string()
}

const CHROME: &str = "0,0,0;4,5,6;0,0,0;4,5,6;0,0,0;7,8,9;0,0,0;0,0,0;1,2,3;0,0,0;1,2,3;1,2,3;4,5,6";

#[test]
fn computed_style_follows_every_attribute_writer() {
    assert_eq!(run(false), CHROME);
}

#[test]
fn computed_style_follows_every_attribute_writer_with_a_custom_element_defined() {
    assert_eq!(run(true), CHROME);
}
