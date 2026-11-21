//! Canvas 2D script surface (web_canvas.js): getContext, the context's
//! attributes as they read back, and the members a charting library calls.
//! Expected values from Chromium 143.0.7499.4 on the same scripts.

use super::*;

const PAGE: &str = "<!DOCTYPE html><html><body><canvas id=c></canvas><canvas id=d width=40 height=20px></canvas></body></html>";

fn bound_with(document: Rc<Document>) -> DomBindings {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings.set_document(document).unwrap();
    bindings
}

fn bound() -> DomBindings {
    bound_with(Rc::new(Document::parse_html(PAGE).unwrap()))
}

fn ev(bindings: &DomBindings, script: &str) -> String {
    match bindings.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => panic!("{script} evaluated to {other:?}"),
    }
}

#[test]
fn canvas_size_reflects_its_attributes() {
    let b = bound();
    assert_eq!(
        ev(&b, "var c = document.getElementById('c'), d = document.getElementById('d'); \
                JSON.stringify([c.width, c.height, d.width, d.height])"),
        "[300,150,40,20]"
    );
    assert_eq!(
        ev(&b, "c.width = 64.7; c.height = '32'; JSON.stringify([c.width, c.getAttribute('width'), c.height])"),
        "[64,\"64\",32]"
    );
}

/// Chromium 143 (a canvas that has a 2D context answers null for any
/// other kind). Here there is no WebGL at all, so a fresh canvas answers
/// null for those kinds too.
#[test]
fn only_a_2d_context_is_handed_out() {
    let b = bound();
    assert_eq!(
        ev(&b, "var c = document.getElementById('c'); var x = c.getContext('2d'); \
                JSON.stringify([x instanceof CanvasRenderingContext2D, c.getContext('webgl'), c.getContext('webgl2'), \
                c.getContext('bitmaprenderer'), c.getContext('2d') === x])"),
        "[true,null,null,null,true]"
    );
    assert_eq!(
        ev(&b, "var d = document.getElementById('d'); JSON.stringify([d.getContext('webgl'), typeof CanvasRenderingContext2D, \
                (function () { try { new CanvasRenderingContext2D(); return 'made'; } catch (e) { return e.name; } })()])"),
        "[null,\"function\",\"TypeError\"]"
    );
}

#[test]
fn font_reads_back_normalized() {
    let b = bound();
    assert_eq!(
        ev(&b, "var x = document.getElementById('c').getContext('2d'); var out = []; \
                ['normal 12px Arial', 'italic 600 1em serif', '12px \"Helvetica Neue\", Arial', 'normal normal 400 14px sans-serif', 'bad', '700 12px Arial'] \
                .forEach(function (f) { x.font = f; out.push(x.font); }); JSON.stringify(out)"),
        r#"["12px Arial","italic 600 16px serif","12px \"Helvetica Neue\", Arial","14px sans-serif","14px sans-serif","bold 12px Arial"]"#
    );
}

#[test]
fn context_attributes_read_back_as_chromium_serializes_them() {
    let b = bound();
    assert_eq!(
        ev(&b, "var x = document.getElementById('c').getContext('2d'); var r = [x.fillStyle, x.strokeStyle, x.lineWidth, x.font, x.textAlign, x.textBaseline, x.globalAlpha, x.lineCap, x.lineJoin]; \
                x.fillStyle = 'rgba(6, 182, 212, 0.1)'; x.strokeStyle = 'RED'; x.lineWidth = 0; x.lineWidth = 3; x.font = 'bold 12px Arial'; \
                x.textAlign = 'middle'; x.textAlign = 'center'; x.globalAlpha = 2; x.globalAlpha = 0.5; x.fillStyle = 'nonsense'; \
                r = r.concat([x.fillStyle, x.strokeStyle, x.lineWidth, x.font, x.textAlign, x.globalAlpha]); JSON.stringify(r)"),
        "[\"#000000\",\"#000000\",1,\"10px sans-serif\",\"start\",\"alphabetic\",1,\"butt\",\"miter\",\
          \"rgba(6, 182, 212, 0.1)\",\"#ff0000\",3,\"bold 12px Arial\",\"center\",0.5]"
    );
}

#[test]
fn save_and_restore_bring_attributes_back() {
    let b = bound();
    assert_eq!(
        ev(&b, "var x = document.getElementById('c').getContext('2d'); x.fillStyle = '#123456'; x.setLineDash([1, 2, 3]); x.save(); \
                x.fillStyle = '#abcdef'; x.setLineDash([]); x.restore(); JSON.stringify([x.fillStyle, x.getLineDash()])"),
        "[\"#123456\",[1,2,3,1,2,3]]"
    );
}

#[test]
fn gradients_and_their_stops() {
    let b = bound();
    assert_eq!(
        ev(&b, "var x = document.getElementById('c').getContext('2d'); var g = x.createLinearGradient(0, 0, 100, 0); \
                g.addColorStop(0, 'red'); var errs = []; \
                try { g.addColorStop(2, 'red'); } catch (e) { errs.push(e.name); } \
                try { g.addColorStop(0.5, 'nope'); } catch (e) { errs.push(e.name); } \
                x.fillStyle = g; JSON.stringify([x.fillStyle === g, g instanceof CanvasGradient, errs])"),
        "[true,true,[\"IndexSizeError\",\"SyntaxError\"]]"
    );
}

/// Not a Chromium value: phase 1 defines the members it draws with and
/// leaves the rest out (web_interfaces.js policy: absent, not stubbed).
#[test]
fn drawing_members_are_there_and_the_rest_absent() {
    let b = bound();
    assert_eq!(
        ev(&b, "var x = document.getElementById('c').getContext('2d'); \
                var have = ['save','restore','scale','rotate','translate','transform','setTransform','resetTransform','fillRect','clearRect','strokeRect', \
                'beginPath','moveTo','lineTo','arc','arcTo','bezierCurveTo','quadraticCurveTo','rect','closePath','fill','stroke','clip', \
                'setLineDash','getLineDash','fillText','strokeText','measureText','createLinearGradient','getImageData','putImageData','createImageData','drawImage','createPattern','createRadialGradient'].filter(function (m) { return typeof x[m] !== 'function'; }); \
                var absent = ['isPointInPath','shadowBlur','globalCompositeOperation'].filter(function (m) { return m in x; }); \
                JSON.stringify([have, absent, typeof document.getElementById('c').toDataURL])"),
        "[[],[],\"undefined\"]"
    );
}

#[test]
fn measure_text_has_a_width() {
    let b = bound();
    assert_eq!(
        ev(&b, "var x = document.getElementById('c').getContext('2d'); x.font = '20px sans-serif'; var m = x.measureText('abcd'); \
                JSON.stringify([m instanceof TextMetrics, m.width > 0])"),
        "[true,true]"
    );
}

/// What the engine reads: the recording, cleared by a clear of the whole
/// bitmap and by a resize.
#[test]
fn draws_are_recorded_for_the_engine() {
    let document = Rc::new(Document::parse_html(PAGE).unwrap());
    let b = bound_with(document.clone());
    b.evaluate("var x = document.getElementById('c').getContext('2d'); x.fillRect(1, 2, 3, 4); x.fillText('hi', 5, 6);").unwrap();
    let node = document.get_element_by_id("c").unwrap().id.raw();
    let snap = b.canvas_snapshot(node).expect("a context");
    assert_eq!((snap.width, snap.height, snap.commands.len()), (300, 150, 2));
    assert!(b.take_canvas_painted());
    b.evaluate("x.clearRect(0, 0, 300, 150); x.fillRect(0, 0, 1, 1);").unwrap();
    assert_eq!(b.canvas_snapshot(node).unwrap().commands.len(), 1);
    b.evaluate("x.canvas.width = 10;").unwrap();
    let snap = b.canvas_snapshot(node).unwrap();
    assert_eq!((snap.width, snap.commands.len()), (10, 0));
}

/// #905 batch 22 red: binding a document is not a canvas paint. The new
/// document gets a full layout and list of its own, so a load (with or
/// without canvases) must not leave the paint mark set: the engine would
/// list and present the same frame a second time after it.
#[test]
fn binding_a_document_is_not_a_paint() {
    let b = bound();
    assert!(!b.take_canvas_painted(), "a fresh document drew nothing");
    b.evaluate("document.getElementById('c').getContext('2d').fillRect(0, 0, 1, 1);").unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    assert!(!b.take_canvas_painted(), "the last document's drawing is moot");
}

/// PR #905 R1 HOLD: a Chart.js-style loop that makes a gradient every
/// frame and clears the whole bitmap holds no more after 10k frames than a
/// frame's worth: no gradient is kept beside the recording. A gradient made
/// before a full clear still paints after it.
#[test]
fn ten_thousand_frames_of_gradients_hold_one_frame() {
    let document = Rc::new(Document::parse_html(PAGE).unwrap());
    let b = bound_with(document.clone());
    b.evaluate(
        "var x = document.getElementById('c').getContext('2d'); var kept = x.createLinearGradient(0, 0, 0, 150); \
         kept.addColorStop(0, 'red'); kept.addColorStop(1, 'blue'); \
         for (var f = 0; f < 10000; f++) { x.clearRect(0, 0, 300, 150); \
           var g = x.createLinearGradient(0, 0, 300, 0); g.addColorStop(0, 'rgba(0,0,255,0.5)'); g.addColorStop(1, '#fff'); \
           x.fillStyle = g; x.fillRect(10, 10, 50 + f % 100, 100); }",
    )
    .unwrap();
    let node = document.get_element_by_id("c").unwrap().id.raw();
    let source = b.canvas_source();
    assert_eq!(b.canvas_snapshot(node).unwrap().commands.len(), 1);
    // The recording, plus the 300x150 bitmap now counted with it.
    assert!(source.retained_bytes() <= 4096 + 300 * 150 * 4, "{} bytes after 10k frames", source.retained_bytes());
    b.evaluate("x.clearRect(0, 0, 300, 150); x.fillStyle = kept; x.fillRect(0, 0, 10, 10);").unwrap();
    let snap = b.canvas_snapshot(node).unwrap();
    match &snap.commands[..] {
        [crate::canvas::DrawCommand::FillRect { style: crate::canvas::CanvasStyle::LinearGradient(g), .. }] => {
            assert_eq!((g.y1, g.stops.len()), (150.0, 2), "{g:?}");
        }
        other => panic!("{other:?}"),
    }
}

/// Canvas size checks, Chrome 148 (tools/parity_oracle/canvas_taint_cases.json
/// "huge-get-image-data", and a probe of oversized canvases):
/// getImageData past the ImageData limit throws RangeError instead of
/// allocating; a canvas over 16384x16384 pixels of area (or a side over the
/// largest bitmap side) has no bitmap: it draws nothing and reads zeros.
#[test]
fn canvas_size_checks_match_chrome() {
    let b = bound();
    assert_eq!(
        ev(&b, "var x = document.getElementById('d').getContext('2d'); \
                try { x.getImageData(0, 0, 100000, 100000); 'no throw' } catch (e) { e.name + ': ' + e.message }"),
        "RangeError: Failed to execute 'getImageData' on 'CanvasRenderingContext2D': Out of memory at ImageData creation"
    );
    assert_eq!(
        ev(&b, "var c = document.createElement('canvas'); c.width = 16384; c.height = 16385; var y = c.getContext('2d'); \
                y.fillStyle = 'red'; y.fillRect(0, 0, 5, 5); Array.from(y.getImageData(0, 0, 1, 1).data).join()"),
        "0,0,0,0"
    );
    assert_eq!(
        ev(&b, "var c = document.createElement('canvas'); c.width = 100000; c.height = 1; var y = c.getContext('2d'); \
                y.fillStyle = 'red'; y.fillRect(0, 0, 5, 5); Array.from(y.getImageData(0, 0, 1, 1).data).join()"),
        "0,0,0,0"
    );
    assert_eq!(
        ev(&b, "var c = document.createElement('canvas'); c.width = 32768; c.height = 1; var y = c.getContext('2d'); \
                y.fillStyle = 'red'; y.fillRect(0, 0, 5, 5); Array.from(y.getImageData(0, 0, 1, 1).data).join()"),
        "255,0,0,255"
    );
}

/// Canvas origin checks, Chrome 148 (tools/parity_oracle/canvas_taint_cases.json,
/// canvas_taint_log.mjs, which serves the page and a second origin from two
/// local servers): after drawing an image from its own origin, another
/// origin, or a data: URL, directly, through a pattern, or through another
/// canvas, what getImageData throws ("ok" for nothing). Here the page is at
/// https://a.test/, the images are `<img>` elements the engine reported
/// loaded from those URLs, and the loader hands back one red pixel.
/// "cors-anonymous-img" is left out: the image loader has no CORS mode, so
/// a crossorigin image is treated as cross-origin (stricter than Chrome).
#[test]
fn canvas_origin_checks_match_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/canvas_taint_cases.json")).unwrap();
    let mut failures = Vec::new();
    for case in data["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        // Redirects are the fetch's (rustkit-engine canvas_context_tests).
        if name == "cors-anonymous-img" || name == "huge-get-image-data" || name.starts_with("redirect-") {
            continue;
        }
        let doc = Rc::new(Document::parse_html(
            "<!DOCTYPE html><html><body><canvas id=c width=8 height=8></canvas><img id=s><img id=o><img id=d></body></html>",
        ).unwrap());
        let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        b.set_location(&url::Url::parse("https://a.test/").unwrap()).unwrap();
        b.set_document(doc.clone()).unwrap();
        b.set_canvas_image_loader(Some(Box::new(|_url: &str, _size: (u32, u32)| {
            rustkit_canvas::CanvasBitmap::from_rgba(1, 1, &[255, 0, 0, 255]).map(|b| (b, Vec::new()))
        })));
        let id = |e: &str| doc.get_element_by_id(e).unwrap().id.raw();
        b.fire_image_load(id("s"), 1, 1, "https://a.test/img.png").unwrap();
        b.fire_image_load(id("o"), 1, 1, "https://b.test/img.png").unwrap();
        b.fire_image_load(id("d"), 1, 1, "data:image/png;base64,AAAA").unwrap();
        let script = case["script"].as_str().unwrap();
        let ours = ev(
            &b,
            &format!(
                "var B = 'https://b.test', x = document.getElementById('c').getContext('2d'), __r = 'none'; \
                 function done(n) {{ __r = n; }} \
                 function read() {{ try {{ x.getImageData(0, 0, 1, 1); done('ok'); }} catch (e) {{ done(e.name); }} }} \
                 function img(src, co, f) {{ f(document.getElementById(src.indexOf(B) === 0 ? 'o' : src.indexOf('data:') === 0 ? 'd' : 's')); }} \
                 {script}; __r"
            ),
        );
        let chrome = case["chrome"]["name"].as_str().unwrap();
        if ours != chrome {
            failures.push(format!("{name}: ours {ours:?}, Chrome {chrome:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

/// Canvas origin checks across redirects, Chrome 148
/// (canvas_taint_cases.json "redirect-same-to-other-origin" /
/// "redirect-same-to-same-origin", and
/// docs/diagnostics/2026-11-16/frame_canvas_cft148.json, where the same
/// four rows are pinned in a frame and at top level): an image of another
/// origin, one the page asked for at its own origin whose response came
/// through another origin, and one asked for at another origin whose
/// response came back to the page's, each leave the canvas not
/// origin-clean; one that stayed at the page's origin does not. The
/// loader reports the response's URLs (each hop and the final one). At
/// top level and in a frame's realm, whose canvases decide with the
/// frame's own origin.
#[test]
fn canvas_origin_checks_follow_the_response_urls() {
    for framed in [false, true] {
        for (hops, expected) in [
            (vec!["https://b.test/img.png"], "SecurityError"),
            (vec!["https://a.test/to-other", "https://b.test/img.png"], "SecurityError"),
            (vec!["https://b.test/back", "https://a.test/img.png"], "SecurityError"),
            (vec!["https://a.test/to-same", "https://a.test/img.png"], "ok"),
        ] {
            let doc = Rc::new(Document::parse_html(
                "<!DOCTYPE html><html><body><canvas id=c width=8 height=8></canvas><img id=s></body></html>",
            ).unwrap());
            let top = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
            // The parent is another origin's page: the frame's own origin decides.
            top.set_location(&url::Url::parse("https://b.test/").unwrap()).unwrap();
            let b = if framed { top.new_child().unwrap() } else { DomBindings::new(JsRuntime::new().unwrap()).unwrap() };
            b.set_location(&url::Url::parse("https://a.test/").unwrap()).unwrap();
            b.set_document(doc.clone()).unwrap();
            let urls: Vec<url::Url> = hops.iter().map(|u| url::Url::parse(u).unwrap()).collect();
            b.set_canvas_image_loader(Some(Box::new(move |_url: &str, _size: (u32, u32)| {
                rustkit_canvas::CanvasBitmap::from_rgba(1, 1, &[255, 0, 0, 255]).map(|bm| (bm, urls.clone()))
            })));
            b.fire_image_load(doc.get_element_by_id("s").unwrap().id.raw(), 1, 1, hops[0]).unwrap();
            let ours = ev(
                &b,
                "var x = document.getElementById('c').getContext('2d'); x.drawImage(document.getElementById('s'), 0, 0); \
                 try { x.getImageData(0, 0, 1, 1); 'ok' } catch (e) { e.name }",
            );
            assert_eq!(ours, expected, "framed {framed}, hops {hops:?}");
        }
    }
}

/// Canvas image sources resolve through the element's loaded resource:
/// what the engine recorded when it told the element its image was in.
/// Properties a page defines on the element (`currentSrc`, `src`,
/// `complete`, `naturalWidth`) are not consulted, and the origin check is
/// made on the resource's URL whether or not its pixels are at hand. At
/// top level and in a frame's realm.
#[test]
fn canvas_image_sources_resolve_through_the_elements_loaded_resource() {
    const PAGE: &str = "<!DOCTYPE html><html><body><canvas id=c width=8 height=8></canvas><canvas id=d width=8 height=8></canvas>\
                        <canvas id=e width=8 height=8></canvas><img id=s><img id=t><img id=u></body></html>";
    for framed in [false, true] {
        let top = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let b = if framed { top.new_child().unwrap() } else { top };
        let doc = Rc::new(Document::parse_html(PAGE).unwrap());
        b.set_location(&url::Url::parse("https://a.test/").unwrap()).unwrap();
        b.set_document(doc.clone()).unwrap();
        let asked = Rc::new(RefCell::new(Vec::<String>::new()));
        let log = asked.clone();
        b.set_canvas_image_loader(Some(Box::new(move |url: &str, _size: (u32, u32)| {
            log.borrow_mut().push(url.to_string());
            let pixel: &[u8] = match url {
                "https://a.test/real.png" => &[255, 0, 0, 255],
                "https://b.test/there.png" => &[0, 0, 255, 255],
                _ => return None,
            };
            rustkit_canvas::CanvasBitmap::from_rgba(1, 1, pixel).map(|bm| (bm, vec![url::Url::parse(url).unwrap()]))
        })));
        let node = |id: &str| doc.get_element_by_id(id).unwrap().id.raw();
        b.fire_image_load(node("s"), 1, 1, "https://a.test/real.png").unwrap();
        b.fire_image_load(node("t"), 1, 1, "https://b.test/absent.png").unwrap();
        let read = "function read(canvas, image) { var x = document.getElementById(canvas).getContext('2d'); x.drawImage(image, 0, 0); \
                    try { var p = x.getImageData(0, 0, 1, 1).data; return p[0] + ',' + p[2] + ',' + p[3]; } catch (e) { return e.name; } }";
        // The element's own image is drawn, whatever the page says its URL is.
        assert_eq!(
            ev(&b, &format!("{read} var s = document.getElementById('s'); \
                 ['currentSrc', 'src'].forEach(function (k) {{ Object.defineProperty(s, k, {{ value: 'https://b.test/there.png' }}); }}); read('c', s)")),
            "255,0,255",
            "framed {framed}"
        );
        assert_eq!(*asked.borrow(), vec!["https://a.test/real.png".to_string()], "framed {framed}: the URLs looked up");
        // An image that has not loaded draws nothing, whatever the page says of it.
        assert_eq!(
            ev(&b, "var u = document.getElementById('u'); Object.defineProperty(u, 'complete', { value: true }); Object.defineProperty(u, 'naturalWidth', { value: 1 }); \
                    Object.defineProperty(u, 'naturalHeight', { value: 1 }); Object.defineProperty(u, 'currentSrc', { value: 'https://b.test/there.png' }); read('e', u)"),
            "0,0,0",
            "framed {framed}"
        );
        assert_eq!(asked.borrow().len(), 1, "framed {framed}: nothing more was looked up");
        // Another origin's image leaves the canvas unreadable, with its pixels at hand or not.
        assert_eq!(ev(&b, "read('d', document.getElementById('t'))"), "SecurityError", "framed {framed}");
    }
}

/// Reading a canvas back and writing it runs no script loop over its
/// pixels: the bytes cross to and from the host whole (IMG-F2). Counted,
/// not timed: with at most 64 turns allowed to any one loop, a 50x50
/// canvas (10,000 bytes) is read, changed and written, and the same for
/// a canvas a frame's document owns.
#[test]
fn a_canvas_is_read_and_written_without_a_loop_over_its_pixels() {
    let script = "var x = document.getElementById('c').getContext('2d'); x.canvas.width = 50; x.canvas.height = 50; \
                  x.fillStyle = 'rgb(1, 2, 3)'; x.fillRect(0, 0, 50, 50); \
                  var i = x.getImageData(0, 0, 50, 50); i.data[0] = 9; x.putImageData(i, 0, 0); \
                  var made = x.createImageData(50, 50), own = new ImageData(50, 50), j = x.getImageData(0, 0, 1, 1).data; \
                  [j[0], j[1], j[2], j[3], i.data.length, made.data.length, own.data.length, i.data instanceof Uint8ClampedArray].join()";
    let b = bound();
    let child = b.new_child().unwrap();
    child.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    // First use installs the canvas script, which has loops of its own.
    ev(&b, "document.getElementById('d').getContext('2d'); 'installed'");
    ev(&child, "document.getElementById('d').getContext('2d'); 'installed'");
    b.set_loop_iteration_limit(64);
    assert_eq!(ev(&b, script), "9,2,3,255,10000,10000,10000,true");
    assert_eq!(ev(&child, script), "9,2,3,255,10000,10000,10000,true");
}

/// CANVAS-SHIM: the members rewritten to call the host directly keep their
/// checks. Too few arguments throws Chrome's TypeError text; an argument
/// passed as undefined counts; a receiver that is not a context is an
/// illegal invocation, for methods and setters alike; arguments convert left
/// to right. Every expected value is Chrome for Testing 148.0.7778.216's on
/// the same script (tools/parity_oracle/canvas_shim_log.mjs, its output in
/// canvas_shim_chrome.json).
#[test]
fn canvas_members_keep_their_argument_and_receiver_checks() {
    let b = bound();
    ev(&b, "var x = document.getElementById('c').getContext('2d'); \
            function msg(f) { try { f(); return 'none'; } catch (e) { return e.name + ': ' + e.message; } } 'ok'");
    for (call, want) in [
        ("x.arc(1, 2, 3, 4)", "TypeError: Failed to execute 'arc' on 'CanvasRenderingContext2D': 5 arguments required, but only 4 present."),
        ("x.moveTo(1)", "TypeError: Failed to execute 'moveTo' on 'CanvasRenderingContext2D': 2 arguments required, but only 1 present."),
        ("x.lineTo()", "TypeError: Failed to execute 'lineTo' on 'CanvasRenderingContext2D': 2 arguments required, but only 0 present."),
        ("x.clearRect(1, 2, 3)", "TypeError: Failed to execute 'clearRect' on 'CanvasRenderingContext2D': 4 arguments required, but only 3 present."),
        ("x.rotate()", "TypeError: Failed to execute 'rotate' on 'CanvasRenderingContext2D': 1 argument required, but only 0 present."),
        ("x.bezierCurveTo(1, 2, 3, 4, 5)", "TypeError: Failed to execute 'bezierCurveTo' on 'CanvasRenderingContext2D': 6 arguments required, but only 5 present."),
        ("x.arc(1, 2, 3, 4, undefined)", "none"),
        ("x.beginPath.call({})", "TypeError: Illegal invocation"),
        ("x.stroke.call(null)", "TypeError: Illegal invocation"),
        ("x.lineTo.call(5, 1, 2)", "TypeError: Illegal invocation"),
        ("Object.getOwnPropertyDescriptor(CanvasRenderingContext2D.prototype, 'lineWidth').set.call({}, 3)", "TypeError: Illegal invocation"),
        ("Object.getOwnPropertyDescriptor(CanvasRenderingContext2D.prototype, 'strokeStyle').set.call({}, 'red')", "TypeError: Illegal invocation"),
        // The receiver is checked before the argument count.
        ("CanvasRenderingContext2D.prototype.arc.call({})", "TypeError: Illegal invocation"),
        ("CanvasRenderingContext2D.prototype.moveTo.call({}, 1)", "TypeError: Illegal invocation"),
        ("CanvasRenderingContext2D.prototype.clearRect.call(null)", "TypeError: Illegal invocation"),
        ("CanvasRenderingContext2D.prototype.translate.call({})", "TypeError: Illegal invocation"),
    ] {
        assert_eq!(ev(&b, &format!("msg(function () {{ {call}; }})")), want, "{call}");
    }
    // Left to right, each once.
    assert_eq!(
        ev(&b, "var seen = []; function n(v) { return { valueOf: function () { seen.push(v); return v; } }; } \
                x.arc(n(1), n(2), n(3), n(4), n(5)); x.lineTo(n(6), n(7)); x.translate(n(8), n(9)); \
                x.lineWidth = n(10); seen.join()"),
        "1,2,3,4,5,6,7,8,9,10"
    );
    // The values still reach the context: a string style reads back serialized, a bad one is ignored.
    assert_eq!(
        ev(&b, "x.strokeStyle = 'RED'; x.fillStyle = 'nope'; x.lineWidth = 4; x.lineWidth = -1; x.lineCap = 'round'; \
                [x.strokeStyle, x.fillStyle, x.lineWidth, x.lineCap].join()"),
        "#ff0000,#000000,4,round"
    );
}

/// CANVAS-SHIM: as WebIDL drops the arguments past a member's own before
/// converting any (Chrome 148: `moveTo(1, 2, o)` never reads `o`;
/// canvas_shim_chrome.json), a path or transform member converts only its
/// own. Was: every path member converted six, scale/rotate/translate six.
#[test]
fn a_canvas_path_member_converts_only_its_own_arguments() {
    let b = bound();
    assert_eq!(
        ev(&b, "var x = document.getElementById('c').getContext('2d'), read = 0; \
                var o = { valueOf: function () { read++; return 1; } }; \
                x.beginPath(o); x.closePath(o, o); x.moveTo(1, 2, o); x.lineTo(1, 2, o, o); x.rect(1, 2, 3, 4, o); \
                x.quadraticCurveTo(1, 2, 3, 4, o); \
                x.save(); x.scale(1, 1, o); x.rotate(0, o); x.translate(0, 0, o); x.restore(); String(read)"),
        "0"
    );
}
