//! The geometry script reads is measured on the first read after a layout,
//! not on every layout (`publish_geometry`, threads plan P2): a page that
//! never reads geometry never pays for it, a read always sees the latest
//! layout, and repeated reads share one snapshot.

use super::*;
use crate::script_net_tests::serve_routes;

fn load(page: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 200,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine
        .execute_script(view, &format!("String({expr})"))
        .unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn builds(engine: &Engine, view: EngineViewId) -> u64 {
    engine.views[&view]
        .bindings
        .as_ref()
        .expect("bindings")
        .geometry_builds()
}

const PAGE: &str =
    "<html><head><style>body{margin:0} #a{width:100px;height:20px} #b{height:30px}</style></head>\
    <body><div id=a></div><div id=b></div></body></html>";

#[test]
fn a_page_that_never_reads_geometry_never_builds_the_snapshot() {
    let (mut engine, view, _server) = load(PAGE);
    assert_eq!(
        builds(&engine, view),
        0,
        "the load laid out without measuring for script"
    );
    // A mutation lays the page out again; still nothing is measured.
    engine
        .execute_script(view, "document.getElementById('a').style.height = '50px'")
        .unwrap();
    engine
        .execute_script(view, "document.getElementById('b').style.height = '60px'")
        .unwrap();
    assert_eq!(builds(&engine, view), 0);
    // The first read measures, once.
    assert_eq!(
        read(&mut engine, view, "document.getElementById('b').offsetTop"),
        "50"
    );
    assert_eq!(builds(&engine, view), 1);
}

#[test]
fn a_read_after_a_mutation_relayout_sees_the_new_geometry() {
    let (mut engine, view, _server) = load(PAGE);
    assert_eq!(
        read(
            &mut engine,
            view,
            "document.getElementById('a').offsetHeight"
        ),
        "20"
    );
    assert_eq!(
        read(
            &mut engine,
            view,
            "document.getElementById('b').getBoundingClientRect().top"
        ),
        "20"
    );
    engine
        .execute_script(view, "document.getElementById('a').style.height = '80px'")
        .unwrap();
    assert_eq!(
        read(
            &mut engine,
            view,
            "document.getElementById('a').offsetHeight"
        ),
        "80"
    );
    assert_eq!(
        read(
            &mut engine,
            view,
            "document.getElementById('b').getBoundingClientRect().top"
        ),
        "80"
    );
    assert_eq!(
        read(
            &mut engine,
            view,
            "getComputedStyle(document.getElementById('a')).height"
        ),
        "80px"
    );
    // A removed element has no box after the relayout, not its old one.
    engine
        .execute_script(view, "document.getElementById('a').style.display = 'none'")
        .unwrap();
    assert_eq!(
        read(
            &mut engine,
            view,
            "document.getElementById('a').offsetHeight"
        ),
        "0"
    );
    assert_eq!(
        read(&mut engine, view, "document.getElementById('b').offsetTop"),
        "0"
    );
}

#[test]
fn reading_geometry_in_a_loop_builds_the_snapshot_once() {
    let (mut engine, view, _server) = load(PAGE);
    let before = builds(&engine, view);
    let sum = read(
        &mut engine,
        view,
        "(function () { var a = document.getElementById('a'), b = document.getElementById('b'), s = 0; \
         for (var i = 0; i < 500; i++) { s += a.getBoundingClientRect().height + b.offsetTop + a.clientWidth \
         + b.scrollHeight + parseFloat(getComputedStyle(a).width); } return s; })()",
    );
    // 20 + 20 + 100 + 30 + 100, 500 times.
    assert_eq!(sum, "135000");
    assert_eq!(
        builds(&engine, view),
        before + 1,
        "one snapshot for every read in the loop"
    );
    // Nothing was laid out since: the next script reads the same snapshot.
    assert_eq!(
        read(
            &mut engine,
            view,
            "document.getElementById('a').offsetWidth"
        ),
        "100"
    );
    assert_eq!(builds(&engine, view), before + 1);
}

#[test]
fn observers_still_report_the_laid_out_rects() {
    let (mut engine, view, _server) = load(
        "<html><head><style>body{margin:0} #spacer{height:300px} #t{height:40px}</style></head><body>\
         <div id=spacer></div><div id=t></div><script>\
         window.__sizes = []; window.__seen = [];\
         new ResizeObserver(function (es) { es.forEach(function (e) { window.__sizes.push(e.contentRect.width + 'x' + e.contentRect.height); }); }).observe(document.getElementById('t'));\
         new IntersectionObserver(function (es) { es.forEach(function (e) { window.__seen.push(e.isIntersecting + '@' + e.boundingClientRect.top); }); }).observe(document.getElementById('t'));\
         </script></body></html>",
    );
    assert_eq!(read(&mut engine, view, "window.__sizes.join()"), "400x40");
    // #t starts at 300 px, below the 200 px viewport.
    assert_eq!(read(&mut engine, view, "window.__seen.join()"), "false@300");
    // A mutation moves #t into view and resizes it: both observers see the new layout.
    engine
        .execute_script(
            view,
            "document.getElementById('spacer').style.height = '100px'; document.getElementById('t').style.height = '60px'",
        )
        .unwrap();
    assert_eq!(
        read(&mut engine, view, "window.__sizes.join()"),
        "400x40,400x60"
    );
    assert_eq!(
        read(&mut engine, view, "window.__seen.join()"),
        "false@300,true@100"
    );
}

/// The snapshot stage on the image- and box-heavy pages in the P2 pull
/// request: the median `snapshot_ms` and `took_ms` of ten mutation
/// relayouts, and the median wall time of a one-element read after each.
/// Not a pass/fail test:
/// `cargo test -p rustkit-engine --features headless --release lazy_geometry_timing -- --ignored --nocapture`
#[test]
#[ignore]
fn lazy_geometry_timing() {
    use crate::relayout_cause_engine_tests::{field_ms, relayout_lines};
    use crate::script_net_tests::serve_byte_routes;
    const PIC: &[u8] =
        include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic1.jpg");
    const CARD: &str = "<div class=card><a href=#>Listing title that wraps a little</a>\
        <span class=price>$12.34</span><span class=ship>Free shipping</span></div>";
    let mut boxes = String::from("<html><body><div id=m></div>");
    for _ in 0..1400 {
        boxes.push_str(CARD);
    }
    boxes.push_str("</body></html>");
    let mut images = String::from("<html><body><div id=m></div>");
    let mut image_routes = Vec::new();
    for i in 0..84 {
        let path: &'static str = Box::leak(format!("/img{i}.jpg").into_boxed_str());
        images.push_str(&format!(
            "<div class=card><img src={path} width=140 height=105>{}</div>",
            &CARD[16..CARD.len() - 6]
        ));
        image_routes.push((path, "image/jpeg", PIC.to_vec()));
    }
    images.push_str("</body></html>");
    image_routes.push(("/", "text/html", images.into_bytes()));
    let fixtures = [
        (
            "box-heavy (1,400 cards)",
            vec![("/", "text/html", boxes.into_bytes())],
        ),
        ("image-heavy (84 <img> cards)", image_routes),
    ];
    let median = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    for (name, routes) in fixtures {
        let server = serve_byte_routes(routes);
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds {
                x: 0,
                y: 0,
                width: 1280,
                height: 800,
            })
            .expect("view");
        let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(engine.load_url(view, url)).expect("load_url");
        let (mut reads, mut lines) = (Vec::new(), Vec::new());
        for i in 0..10 {
            lines.extend(relayout_lines(|| {
                engine
                    .execute_script(
                        view,
                        &format!("document.getElementById('m').className = 'x{i}'"),
                    )
                    .expect("mutation");
            }));
            let started = std::time::Instant::now();
            engine
                .execute_script(
                    view,
                    "document.getElementById('m').getBoundingClientRect().top",
                )
                .expect("read");
            reads.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        let field = |f: &str| median(lines.iter().filter_map(|l| field_ms(l, f)).collect());
        println!(
            "{name}: relayouts {}, median snapshot_ms {:.1}, took_ms {:.1}; first read after each {:.1} ms",
            lines.len(),
            field("snapshot_ms"),
            field("took_ms"),
            median(reads)
        );
    }
}
