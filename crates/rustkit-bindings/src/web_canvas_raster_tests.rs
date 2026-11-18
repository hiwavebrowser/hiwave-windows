//! Canvas 2D pixels (STR-7): each script in
//! tools/parity_oracle/canvas_raster_cases.json draws on a fresh 40x40
//! canvas and `getImageData` reads it back; the pixels are compared with
//! Chrome for Testing 148's (canvas_raster_log.mjs).
//!
//! Chrome rasterizes with Skia and the engine with tiny-skia, a port of
//! Skia's CPU pipeline. Covered and uncovered pixels match exactly; edge
//! pixels differ, because Chrome anti-aliases analytically and tiny-skia
//! by 4x supersampling, which places an edge to a quarter pixel. So pixels
//! are compared premultiplied, and a case passes when no pixel is off by
//! more than [`WRONG`] (a quarter pixel of coverage and rounding: more is a
//! wrong colour, or a missed or extra pixel) and the mean difference over
//! the pixels Chrome painted is at most [`MEAN`].

use super::*;

/// A pixel this close counts as the same (reported, not asserted).
const EDGE: u8 = 2;
/// Channel difference no pixel may show.
const WRONG: u8 = 80;
/// Mean of each pixel's largest channel difference, over Chrome's painted
/// pixels (thin curved strokes, nearly all edge, are the worst: ~13).
const MEAN: f32 = 14.0;

pub(crate) struct Compared {
    pub(crate) wrong: usize,
    pub(crate) off: usize,
    pub(crate) painted: usize,
    pub(crate) max: u8,
    /// Sum of every pixel's largest channel difference.
    pub(crate) sum: u64,
}

pub(crate) fn compare(ours: &[u8], chrome: &[u8]) -> Compared {
    // Premultiplied, as the pixels are seen: an edge pixel of alpha 3
    // unpremultiplies to a colour far from its neighbours' in either browser.
    fn pm(p: &[u8]) -> [u8; 4] {
        let f = |c: u8| ((c as u32 * p[3] as u32 + 127) / 255) as u8;
        [f(p[0]), f(p[1]), f(p[2]), p[3]]
    }
    let mut c = Compared { wrong: 0, off: 0, painted: 0, max: 0, sum: 0 };
    for (a, b) in ours.chunks(4).zip(chrome.chunks(4)) {
        if b[3] != 0 {
            c.painted += 1;
        }
        let (a, b) = (pm(a), pm(b));
        let d = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        c.max = c.max.max(d);
        c.sum += d as u64;
        if d > WRONG {
            c.wrong += 1;
        } else if d > EDGE {
            c.off += 1;
        }
    }
    c
}

pub(crate) fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect()
}

/// Runs `script` (the context is `x`) on a fresh `w`x`h` canvas and reads
/// it back with `getImageData`, as hex RGBA.
pub(crate) fn draw(script: &str, w: u32, h: u32) -> String {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings
        .set_document(Rc::new(Document::parse_html("<!DOCTYPE html><html><body></body></html>").unwrap()))
        .unwrap();
    let js = format!(
        "(function () {{ var cv = document.createElement('canvas'); cv.width = {w}; cv.height = {h}; \
         document.body.appendChild(cv); var x = cv.getContext('2d'); \
         (function (x) {{ {script} }})(x); \
         var d = x.getImageData(0, 0, {w}, {h}).data, hex = ''; \
         for (var i = 0; i < d.length; i++) hex += (d[i] < 16 ? '0' : '') + d[i].toString(16); \
         return hex; }})()"
    );
    match bindings.evaluate(&js) {
        Ok(JsValue::String(s)) => s,
        other => panic!("{script} evaluated to {other:?}"),
    }
}

pub(crate) fn check_file(json: &str) {
    let data: serde_json::Value = serde_json::from_str(json).unwrap();
    let w = data["size"][0].as_u64().unwrap() as u32;
    let h = data["size"][1].as_u64().unwrap() as u32;
    let mut failures = Vec::new();
    for case in data["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let chrome = unhex(case["chrome_rgba"].as_str().unwrap());
        let ours = unhex(&draw(case["script"].as_str().unwrap(), w, h));
        assert_eq!(ours.len(), chrome.len(), "{name}: getImageData length");
        let c = compare(&ours, &chrome);
        if std::env::var("STR7_DUMP").as_deref() == Ok(name) {
            for y in 0..h as usize {
                let row: Vec<String> = (0..w as usize)
                    .map(|x| {
                        let i = (y * w as usize + x) * 4;
                        format!("{:3}/{:3}", ours[i + 3], chrome[i + 3])
                    })
                    .collect();
                eprintln!("{y:2} {}", row.join(" "));
            }
        }
        let mean = c.sum as f32 / c.painted.max(1) as f32;
        eprintln!("{name}: painted {} off {} wrong {} max {} mean {mean:.2}", c.painted, c.off, c.wrong, c.max);
        if c.wrong > 0 || mean > MEAN {
            failures.push(format!("{name}: {} wrong, {} off, max diff {}, mean {mean:.2}", c.wrong, c.off, c.max));
        }
    }
    assert!(failures.is_empty(), "pixels unlike Chrome 148:\n{}", failures.join("\n"));
}

#[test]
fn canvas_pixels_match_chrome() {
    check_file(include_str!("../../../tools/parity_oracle/canvas_raster_cases.json"));
}

/// Slice 2: clip, gradients, patterns, drawImage of a canvas, putImageData.
#[test]
fn canvas_pixels_match_chrome_slice_2() {
    check_file(include_str!("../../../tools/parity_oracle/canvas_raster2_cases.json"));
}
