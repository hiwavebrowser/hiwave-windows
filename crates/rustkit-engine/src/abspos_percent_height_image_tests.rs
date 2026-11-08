//! A percentage height on an absolutely positioned replaced element whose
//! containing block is itself an `absolute; inset: 0` layer (its height
//! comes from the insets, not from a `height` declaration).
//!
//! railway.com's hero image is `absolute inset-0 w-full h-full object-cover`
//! inside an `absolute inset-0` layer 1700 px tall: Chrome 148 draws it
//! 1248x1700, RustKit 1248x1248 (the image's aspect ratio wins, so the
//! bottom of the hero has no picture). netflix's hero layer has the same
//! shape. Pete's hand test 10-27. `b` is a control that already matches.
//!
//! Expected row from the pinned Chrome 148.

use super::*;

const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

const CHROME: &str = "a:0:0:600:400 b:0:410:600:400 c:0:820:600:400";

const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('img'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

fn page() -> String {
    format!(
        "<!doctype html><html><head><style>\
         body {{ margin: 0 }}\
         .box {{ position: relative; width: 600px; height: 400px; margin-bottom: 10px }}\
         .layer {{ position: absolute; inset: 0 }}\
         img {{ display: block }}\
         .a {{ position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover }}\
         .b {{ position: absolute; width: 100%; height: 100% }}\
         .c {{ position: absolute; top: 0; left: 0; width: 100%; height: 100% }}\
         </style></head><body>\
         <div class=box><div class=layer><img id=a class=a src='{PNG}'></div></div>\
         <div class=box><img id=b class=b src='{PNG}'></div>\
         <div class=box><div class=layer><img id=c class=c src='{PNG}'></div></div>\
         </body></html>"
    )
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_percent_height_image_in_an_inset_sized_layer_fills_the_layer() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 1280, 1300))
        .expect("headless view");
    engine.load_html(id, &page()).expect("load_html");
    let value = engine.execute_script(id, BOXES).expect("script");
    let got = value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value);
    assert_eq!(got, CHROME);
}

/// `getBoundingClientRect` of every `<img>` on `html`, as `id:x:y:w:h`
/// rows, from a 1280x1300 headless view.
#[cfg(all(target_os = "macos", feature = "headless"))]
fn image_rects(html: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 1280, 1300))
        .expect("headless view");
    engine.load_html(id, html).expect("load_html");
    let value = engine.execute_script(id, BOXES).expect("script");
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

/// The `top` / `left` of a positioned `<img>` were ignored: the box never
/// took its positioning, so the image sat at its static position. Chrome:
/// `d:70:50:100:50`.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn an_abspos_image_is_placed_by_its_top_and_left() {
    let html = format!(
        "<!doctype html><html><head><style>\
         body {{ margin: 0 }}\
         .box {{ position: relative; width: 600px; height: 400px }}\
         img {{ display: block }}\
         .d {{ position: absolute; top: 50px; left: 70px; width: 100px; height: 50px }}\
         </style></head><body>\
         <div class=box><div style='height: 30px'></div><img id=d class=d src='{PNG}'></div>\
         </body></html>"
    );
    assert_eq!(image_rects(&html), "d:70:50:100:50");
}

/// `width: 100%` on an abspos image inside an `inset: 0` layer is the
/// layer's width, and the auto height follows the 1:1 ratio (`w`). An
/// abspos REPLACED box with `inset: 0` and no size keeps its intrinsic size
/// (CSS 2.1 §10.3.8, §10.6.5): the 1x1 PNG stays 1x1 (`n`), where a block
/// would stretch to 600x400. Chrome: `n:0:0:1:1 w:0:410:600:600`.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn an_abspos_image_width_percent_resolves_against_the_inset_layer() {
    let html = format!(
        "<!doctype html><html><head><style>\
         body {{ margin: 0 }}\
         .box {{ position: relative; width: 600px; height: 400px; margin-bottom: 10px }}\
         .layer {{ position: absolute; inset: 0 }}\
         img {{ display: block }}\
         .n {{ position: absolute; inset: 0 }}\
         .w {{ position: absolute; top: 0; left: 0; width: 100% }}\
         </style></head><body>\
         <div class=box><div class=layer><img id=n class=n src='{PNG}'></div></div>\
         <div class=box><div class=layer><img id=w class=w src='{PNG}'></div></div>\
         </body></html>"
    );
    assert_eq!(image_rects(&html), "n:0:0:1:1 w:0:410:600:600");
}

/// A `position: static` image is the box it always was: it keeps the flow
/// (`e` sits under the 20px block, `f` on the line after it), and its
/// layout box carries no positioning at all. The box-level check is the
/// byte-for-byte guard: `LayoutBox::new` sets exactly these fields, and a
/// static, floated or relative `<img>` must still leave them untouched. Only
/// the absolute one differs, and it carries its offsets.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_static_image_is_unchanged() {
    let html = format!(
        "<!doctype html><html><head><style>\
         body {{ margin: 0 }}\
         .box {{ position: relative; width: 600px; height: 400px }}\
         .e {{ display: block; width: 100px; height: 50px }}\
         .f {{ width: 40px; height: 40px }}\
         .r {{ position: relative; top: 5px; width: 40px; height: 40px }}\
         .fl {{ float: left; width: 40px; height: 40px }}\
         .a {{ position: absolute; top: 1px; left: 2px; z-index: 3; width: 40px; height: 40px }}\
         </style></head><body>\
         <div class=box><div style='height: 20px'></div><img id=e class=e src='{PNG}'><img id=f class=f src='{PNG}'></div>\
         <div><img id=r class=r src='{PNG}'><img id=fl class=fl src='{PNG}'><img id=a class=a src='{PNG}'></div>\
         </body></html>"
    );
    let got = image_rects(&html);
    assert!(
        got.starts_with("e:0:20:100:50 f:0:70:40:40 "),
        "static images keep their flow geometry: {got}"
    );

    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let document = Rc::new(Document::parse_html(&html).expect("parse"));
    let layout = engine.build_layout_from_document(&document, &[]);
    fn image(b: &LayoutBox, id: &str) -> Option<LayoutBox> {
        let selector = format!("#{id}");
        if matches!(b.box_type, BoxType::Image { .. })
            && b.identity.as_ref().is_some_and(|i| i.selector == selector)
        {
            return Some(b.clone());
        }
        b.children.iter().find_map(|c| image(c, id))
    }
    for id in ["e", "f", "r", "fl"] {
        let b = image(&layout, id).unwrap_or_else(|| panic!("no #{id}"));
        assert_eq!(b.position, rustkit_layout::Position::Static, "#{id}");
        assert_eq!(b.float, rustkit_css::Float::None, "#{id}");
        assert_eq!(b.clear, rustkit_css::Clear::None, "#{id}");
        let o = &b.offsets;
        assert_eq!((o.top, o.right, o.bottom, o.left), (None, None, None, None), "#{id}");
        assert_eq!(b.z_index, 0, "#{id}");
    }
    let a = image(&layout, "a").expect("no #a");
    assert_eq!(a.position, rustkit_layout::Position::Absolute);
    assert_eq!((a.offsets.top, a.offsets.left), (Some(1.0), Some(2.0)));
    assert_eq!(a.z_index, 3);
}

/// A positioned image paints in the positioned bucket: with `z-index: 1`
/// it is over the later static sibling that covers the same spot, with
/// `z-index: -1` under it. Hit-testing reads the same paint order, so the
/// point lookup names the image in the first box and the sibling in the
/// second. Before the fix both boxes hit the sibling.
#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_positioned_image_with_a_z_index_paints_over_a_later_sibling() {
    let html = format!(
        "<!doctype html><html><head><style>\
         body {{ margin: 0 }}\
         .box {{ position: relative; width: 600px; height: 400px }}\
         img {{ position: absolute; top: 0; left: 0; width: 100px; height: 100px }}\
         .later {{ width: 100px; height: 100px; background: red }}\
         </style></head><body>\
         <div class=box><img id=over style='z-index: 1' src='{PNG}'><div id=later1 class=later></div></div>\
         <div class=box><img id=under style='z-index: -1' src='{PNG}'><div id=later2 class=later></div></div>\
         </body></html>"
    );
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds::new(0, 0, 1280, 1300))
        .expect("headless view");
    engine.load_html(view, &html).expect("load_html");
    let node = |selector: &str| {
        engine
            .element_diagnostics(
                view,
                &DiagnosticTarget::Selector(selector.into()),
                &DiagnosticsOptions::default(),
            )
            .unwrap()
            .unwrap_or_else(|| panic!("no {selector}"))
            .identity
            .node_id
    };
    assert_eq!(engine.devtools_node_at(view, 50.0, 50.0).unwrap(), Some(node("#over")));
    assert_eq!(engine.devtools_node_at(view, 50.0, 450.0).unwrap(), Some(node("#later2")));
}
