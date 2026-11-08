//! Image decode off the page thread (threads plan, step D1).
//!
//! Until the decode pool, a fetched image was decoded inside the future
//! that fetched it, on the page thread, between script and layout. Now the
//! bytes go to a `rustkit-decode` thread and the fetch future waits for the
//! pixels without holding the thread. What the page sees must not change:
//! layout runs without the image, and its arrival lays out again through
//! the same `image_loaded` path. A decode that comes back after the view
//! navigated on (or was stopped) is dropped.

use super::*;
use crate::script_net_tests::serve_byte_routes;

/// 640 x 480 JPEG.
const JPEG: &[u8] =
    include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic4.jpg");

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn engine_and_view() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 200,
        })
        .expect("view");
    (engine, view)
}

fn image_relayouts(engine: &Engine, view: EngineViewId) -> u32 {
    engine
        .relayout_stats(view)
        .expect("view")
        .by_cause
        .get("image_loaded")
}

#[test]
fn a_loaded_raster_image_is_decoded_by_the_pool_and_lays_out_again() {
    let server = serve_byte_routes(vec![
        (
            "/",
            "text/html",
            b"<html><body><img id=i src=/a.jpg></body></html>".to_vec(),
        ),
        ("/a.jpg", "image/jpeg", JPEG.to_vec()),
    ]);
    let (mut engine, view) = engine_and_view();
    let base = format!("http://127.0.0.1:{}", server.port);
    runtime()
        .block_on(engine.load_url(view, Url::parse(&format!("{base}/")).unwrap()))
        .expect("load_url");

    let image = Url::parse(&format!("{base}/a.jpg")).unwrap();
    assert_eq!(engine.get_image_dimensions(&image), Some((640, 480)));
    let decodes = engine.image_manager().decode_stats();
    assert_eq!(decodes.pooled, 1, "{decodes:?}");
    assert_eq!(decodes.inline, 0, "{decodes:?}");
    // The same relayout as before the pool: one, for the one image.
    let stats = engine.relayout_stats(view).expect("view").clone();
    assert_eq!(stats.by_cause.get("image_loaded"), 1, "{stats:?}");
    assert_eq!(
        stats.last,
        Some(RelayoutCause::ImageLoaded { count: 1 }),
        "{stats:?}"
    );
    assert_eq!(
        engine
            .execute_script(
                view,
                "document.getElementById('i').getBoundingClientRect().width"
            )
            .unwrap(),
        "Number(640.0)",
        "laid out at the decoded size"
    );
}

#[test]
fn a_raster_image_that_fails_to_decode_is_left_out_as_before() {
    let server = serve_byte_routes(vec![
        (
            "/",
            "text/html",
            b"<html><body><img src=/bad.png></body></html>".to_vec(),
        ),
        (
            "/bad.png",
            "image/png",
            b"\x89PNG\r\n\x1a\nnot really".to_vec(),
        ),
    ]);
    let (mut engine, view) = engine_and_view();
    let base = format!("http://127.0.0.1:{}", server.port);
    runtime()
        .block_on(engine.load_url(view, Url::parse(&format!("{base}/")).unwrap()))
        .expect("a bad image does not fail the load");
    assert!(!engine.is_image_cached(&Url::parse(&format!("{base}/bad.png")).unwrap()));
    assert_eq!(engine.image_manager().decode_stats().pooled, 1);
    assert_eq!(
        image_relayouts(&engine, view),
        0,
        "nothing arrived, nothing to lay out"
    );
}

/// After the load, a timer adds an image the server holds for 300 ms
/// (`/held...`): the live loop fetches it across turns.
fn live_image_page() -> (
    Engine,
    EngineViewId,
    Url,
    crate::script_net_tests::Server,
    tokio::runtime::Runtime,
) {
    let server = serve_byte_routes(vec![
        (
            "/",
            "text/html",
            b"<html><body><p>x</p><script>setTimeout(function () { var i = document.createElement('img'); \
              i.src = '/held.jpg'; document.body.appendChild(i); }, 60000);</script></body></html>"
                .to_vec(),
        ),
        ("/held.jpg", "image/jpeg", JPEG.to_vec()),
    ]);
    let (mut engine, view) = engine_and_view();
    let base = format!("http://127.0.0.1:{}", server.port);
    let rt = runtime();
    rt.block_on(engine.load_url(view, Url::parse(&format!("{base}/")).unwrap()))
        .expect("load_url");
    let image = Url::parse(&format!("{base}/held.jpg")).unwrap();
    (engine, view, image, server, rt)
}

/// Turns the live loop until nothing is in flight.
fn settle(engine: &mut Engine, view: EngineViewId, rt: &tokio::runtime::Runtime) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while rt.block_on(engine.pump_live(view, 10)).in_flight > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the live image never settled"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn a_live_image_arrives_through_the_pool_and_lays_out_again() {
    let (mut engine, view, image, _server, rt) = live_image_page();
    let turn = rt.block_on(engine.pump_live(view, 60_000));
    assert_eq!(
        turn.in_flight, 1,
        "the turn did not wait for the image: {turn:?}"
    );
    assert_eq!(image_relayouts(&engine, view), 0);
    settle(&mut engine, view, &rt);
    assert!(engine.is_image_cached(&image));
    assert_eq!(engine.image_manager().decode_stats().pooled, 1);
    assert_eq!(image_relayouts(&engine, view), 1);
}

#[test]
fn a_live_image_whose_navigation_was_stopped_is_dropped() {
    let (mut engine, view, image, _server, rt) = live_image_page();
    let turn = rt.block_on(engine.pump_live(view, 60_000));
    assert_eq!(turn.in_flight, 1, "{turn:?}");
    // Stop moves the view's navigation on while the server still holds
    // the image.
    assert!(engine.stop(view));
    settle(&mut engine, view, &rt);
    assert!(
        !engine.is_image_cached(&image),
        "a superseded decode is never cached"
    );
    let decodes = engine.image_manager().decode_stats();
    assert_eq!(decodes.superseded, 1, "{decodes:?}");
    assert_eq!(
        image_relayouts(&engine, view),
        0,
        "nothing arrived for the page"
    );
}

/// The image-heavy timing in the D1 pull request: 80 JPEGs (the five MDN
/// gallery pictures under sixteen URLs each), as eBay search shows 84.
/// Reports the load's wall time and the page thread's longest hold, read
/// by a 1 ms ticker on the same runtime. Not a pass/fail test:
/// `cargo test -p rustkit-engine --features headless --release decode_pool_timing -- --ignored --nocapture`
#[test]
#[ignore]
fn decode_pool_timing() {
    const PICS: [&[u8]; 5] = [
        include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic1.jpg"),
        include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic2.jpg"),
        include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic3.jpg"),
        include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic4.jpg"),
        include_bytes!("../../../websuite/js-ladder/01-mdn/32-image-gallery/images/pic5.jpg"),
    ];
    const COUNT: usize = 80;
    let mut page = String::from("<html><body>");
    let mut routes = Vec::new();
    for i in 0..COUNT {
        let path: &'static str = Box::leak(format!("/img{i}.jpg").into_boxed_str());
        page.push_str(&format!("<img src={path} width=100 height=75>"));
        routes.push((path, "image/jpeg", PICS[i % PICS.len()].to_vec()));
    }
    page.push_str("</body></html>");
    routes.push(("/", "text/html", page.into_bytes()));
    let server = serve_byte_routes(routes);

    for run in 0..3 {
        let (mut engine, view) = engine_and_view();
        let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
        let rt = runtime();
        let started = std::time::Instant::now();
        let (longest, janky) = rt.block_on(async {
            let done = std::cell::Cell::new(false);
            let ticker = async {
                let (mut last, mut longest, mut janky) =
                    (std::time::Instant::now(), std::time::Duration::ZERO, 0);
                while !done.get() {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    let gap = last.elapsed();
                    longest = longest.max(gap);
                    janky += usize::from(gap > std::time::Duration::from_millis(50));
                    last = std::time::Instant::now();
                }
                (longest, janky)
            };
            let load = async {
                engine.load_url(view, url).await.expect("load_url");
                done.set(true);
            };
            futures::future::join(load, ticker).await.1
        });
        let loaded = (0..COUNT)
            .filter(|i| {
                engine.is_image_cached(
                    &Url::parse(&format!("http://127.0.0.1:{}/img{i}.jpg", server.port)).unwrap(),
                )
            })
            .count();
        println!(
            "run {run}: load {:?}, longest page-thread hold {longest:?}, holds over 50 ms {janky}, images cached {loaded}/{COUNT}",
            started.elapsed()
        );
    }
}
