//! `document.elementFromPoint` uses the layout hit test once a page is laid out.

use super::*;

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

#[test]
fn element_from_point_hits_the_topmost_laid_out_box() {
    let page = "<html><head><style>body{margin:0}#a{position:absolute;left:10px;top:20px;width:100px;height:40px}#b{position:absolute;left:20px;top:30px;width:30px;height:10px}</style></head>\
        <body><div id=a></div><div id=b></div></body></html>";
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    assert_eq!(read(&mut engine, view, "document.elementFromPoint(25, 35) && document.elementFromPoint(25, 35).id"), "b");
    assert_eq!(read(&mut engine, view, "document.elementFromPoint(15, 25) && document.elementFromPoint(15, 25).id"), "a");
    assert_eq!(read(&mut engine, view, "document.elementFromPoint(-5, 10)"), "null");
    let chain = read(&mut engine, view, "document.elementsFromPoint(25, 35).map(function (e) { return e.id || e.localName; }).join()");
    assert!(chain.starts_with("b,"), "{chain}");
}

#[test]
fn border_radius_is_resolved_px_and_the_point_query_stays_finite() {
    let page = "<html><head><style>\
        body{margin:0}\
        #a{position:absolute;left:10px;top:20px;width:100px;height:40px}\
        #b{position:absolute;left:20px;top:30px;width:80px;height:40px;border-radius:10px 20px}\
        #c{position:absolute;left:20px;top:80px;width:80px;height:40px;border-radius:10px / 4px}\
        #d{position:absolute;left:20px;top:140px;width:100px;height:50px;border-radius:50%}\
        #e{position:absolute;left:20px;top:200px;width:40px;height:40px;font-size:20px;border-radius:1em}\
        </style></head><body><div id=a></div><div id=b></div><div id=c></div><div id=d></div><div id=e></div></body></html>";
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = read(
        &mut engine,
        view,
        "(function () {\
            function radii(id) {\
                var c = getComputedStyle(document.getElementById(id));\
                return [c.borderRadius, c.borderTopLeftRadius, c.borderTopRightRadius, c.borderBottomRightRadius, c.borderBottomLeftRadius].join(' ');\
            }\
            var e = document.elementFromPoint(25, 35);\
            var half = Number.parseInt(getComputedStyle(e).borderRadius) / 2;\
            var again = document.elementFromPoint(25, 35 + half);\
            var r = document.getElementById('a').getBoundingClientRect();\
            var finite = [r.x, r.y, r.width, r.height, r.top, r.right, r.bottom, r.left].every(Number.isFinite);\
            return [radii('a'), radii('b'), radii('c'), radii('d'), radii('e'), String(half), again && again.id, String(finite)].join('|');\
        })()",
    );
    assert_eq!(
        got,
        "0px 0px 0px 0px 0px|10px 20px 10px 20px 10px 20px|10px / 4px 10px 4px 10px 4px 10px 4px 10px 4px|50px / 25px 50px 25px 50px 25px 50px 25px 50px 25px|20px 20px 20px 20px 20px|5|b|true",
        "{got}"
    );
}
