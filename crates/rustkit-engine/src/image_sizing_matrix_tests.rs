//! IMG1: how an `<img>` is sized, against Chrome for Testing 148.0.7778.216.
//!
//! Two pages in `docs/diagnostics/2026-11-11/img1/`, each measured at
//! device pixel ratio 1 and 2 in an 800 x 600 viewport (`img1_probe.cjs`
//! prints the lines kept in `*.chrome148.txt`), with images of 100x50,
//! 200x100 and 400x200 pixels:
//! - `img1_matrix.html` (m1..m30): `srcset` with `w` descriptors and `sizes`
//!   (vw, px, `calc()`, a media-conditioned list, none, `auto`), `x`
//!   descriptors, `<picture><source>`, `width`/`height` attributes, CSS
//!   sizes, `object-fit`, `aspect-ratio`, flex and grid items;
//! - `img1_attrs.html` (a1..a24): `width="300" height="100"` attributes (a
//!   3:1 ratio on a 2:1 image) against author CSS for one or both
//!   dimensions.
//!
//! A cell is `id:WxH:pN:nWxH`: the border box, which image was picked
//! (`currentSrc`), and `naturalWidth` x `naturalHeight`. The box-and-pick
//! part and the natural-size part are compared separately, so a cell with
//! a right box and a wrong `naturalWidth` is one gap, not two.

use super::*;
use crate::script_net_tests::serve_byte_routes;

const READ: &str = "Array.prototype.map.call(document.querySelectorAll('img'), function (e) {\
  var r = e.getBoundingClientRect(); var m = /i(\\d+)\\.png/.exec(e.currentSrc);\
  return e.id + ':' + Math.round(r.width) + 'x' + Math.round(r.height) + ':p' + (m ? m[1] : 0) + ':n' + e.naturalWidth + 'x' + e.naturalHeight;\
}).join(' ')";

/// Cells that do not match Chrome yet, as `page@dpr:id:part` (`part` is
/// `box` or `nat`). A listed cell that starts to match fails the test, so
/// the list cannot go stale.
const GAPS: &[&str] = &[
    // `sizes="auto"` on a lazy image with `width="300"`: Chrome's source
    // size is the image's own 300px width, so `naturalWidth` is 300. The
    // engine does not know `auto` and uses 100vw (800). The box and the
    // pick are right; only script sees it.
    "matrix@1:m22:nat",
    "matrix@2:m22:nat",
];

fn cells(page: &'static [u8], dpr: f32) -> Vec<String> {
    let server = serve_byte_routes(vec![
        ("/", "text/html", page.to_vec()),
        (
            "/i100.png",
            "image/png",
            include_bytes!("../../../docs/diagnostics/2026-11-11/img1/i100.png").to_vec(),
        ),
        (
            "/i200.png",
            "image/png",
            include_bytes!("../../../docs/diagnostics/2026-11-11/img1/i200.png").to_vec(),
        ),
        (
            "/i400.png",
            "image/png",
            include_bytes!("../../../docs/diagnostics/2026-11-11/img1/i400.png").to_vec(),
        ),
    ]);
    let config = EngineConfig {
        device_pixel_ratio: dpr,
        ..EngineConfig::default()
    };
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let out = engine.execute_script(view, READ).expect("read");
    out.strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&out)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn check(name: &str, page: &'static [u8], chrome: &str) {
    let mut wrong = Vec::new();
    let mut gaps_that_pass = Vec::new();
    for line in chrome.lines() {
        let mut expected = line.split_whitespace();
        let dpr: f32 = expected
            .next()
            .and_then(|d| d.strip_prefix("dpr"))
            .and_then(|d| d.parse().ok())
            .expect("dprN");
        let got = cells(page, dpr);
        let expected: Vec<&str> = expected.collect();
        assert_eq!(got.len(), expected.len(), "{name}@{dpr}: the page's images");
        for (want, got) in expected.iter().zip(&got) {
            let id = want.split(':').next().unwrap_or("");
            let (want_box, want_nat) = want.rsplit_once(":n").expect("cell");
            let (got_box, got_nat) = got.rsplit_once(":n").expect("cell");
            for (part, want, got) in [("box", want_box, got_box), ("nat", want_nat, got_nat)] {
                let key = format!("{name}@{dpr}:{id}:{part}");
                match (want == got, GAPS.contains(&key.as_str())) {
                    (false, false) => wrong.push(format!("{key}: engine {got}, Chrome {want}")),
                    (true, true) => gaps_that_pass.push(key),
                    _ => {}
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    assert!(
        gaps_that_pass.is_empty(),
        "listed as a gap but matches Chrome now, take it off the list: {gaps_that_pass:?}"
    );
}

#[test]
fn srcset_sizes_and_css_sized_images_are_as_in_chrome() {
    check(
        "matrix",
        include_bytes!("../../../docs/diagnostics/2026-11-11/img1/img1_matrix.html"),
        include_str!("../../../docs/diagnostics/2026-11-11/img1/img1_matrix.chrome148.txt"),
    );
}

#[test]
fn width_and_height_attributes_against_author_css_are_as_in_chrome() {
    check(
        "attrs",
        include_bytes!("../../../docs/diagnostics/2026-11-11/img1/img1_attrs.html"),
        include_str!("../../../docs/diagnostics/2026-11-11/img1/img1_attrs.chrome148.txt"),
    );
}
