//! GPU canvas, A3-style: the GPU picture of every canvas raster pin
//! (tools/parity_oracle/canvas_raster_cases.json and canvas_raster2_cases.json,
//! Chrome for Testing 148.0.7778.216, 40 x 40) against Chrome's pixels,
//! the script run through the real bindings with the GPU switch on.
//!
//! Bounds, both gated per case:
//! - the CPU bitmap's own pin bounds against Chrome (rustkit-bindings
//!   web_canvas_raster_tests.rs: no premultiplied channel off by more than
//!   80, mean difference at most 14 over Chrome's painted pixels);
//! - the GPU picture's own bound (A3, pinned in
//!   tools/parity_oracle/canvas_raster_gpu_bounds.json): 1.25x the further
//!   of Chrome's own GPU canvas (canvas_raster_gpu_log.mjs) and the CPU
//!   bitmap from Chrome's software raster on that case, by the same metric,
//!   at least max 2 / mean 0.5 (rounding).
//!
//! Gated: every gradient case is drawn by the GPU (not a reset from the
//! CPU bitmap), and every case the GPU draws is within both bounds
//! (GPU-AA: under 4x MSAA thin curved strokes were past them).
//! Cases that draw an image or a pattern are not run here: this harness has
//! no image loader (the bitmap pins load them); a case the GPU does not
//! draw yet is reported as "cpu".

use super::*;
use rustkit_bindings::canvas::{set_gpu_canvas, GpuReset};
use rustkit_bindings::DomBindings;
use rustkit_js::JsRuntime;

const KEY: &str = "rustkit-canvas:9@gchrome";
const WRONG: u8 = 80;
const MEAN: f32 = 14.0;

/// The case's A3 bound (max, mean) from the pinned distances.
fn own_bound(bounds: &serde_json::Value, name: &str) -> Option<(u8, f32)> {
    let c = &bounds["cases"][name];
    let get = |k: &str, f: &str| c[k][f].as_f64();
    let max = get("chrome_gpu", "max")?.max(get("cpu", "max")?);
    let mean = get("chrome_gpu", "mean")?.max(get("cpu", "mean")?);
    Some(((1.25 * max).max(2.0).min(WRONG as f64) as u8, (1.25 * mean).max(0.5).min(MEAN as f64) as f32))
}

fn gpu() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor { backends: wgpu::Backends::all(), ..Default::default() });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("canvas gpu chrome test device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
        },
        None,
    ))
    .ok()?;
    Some((Arc::new(device), Arc::new(queue)))
}

fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect()
}

/// (max premultiplied channel difference, mean over Chrome's painted
/// pixels, pixels over 2) of premultiplied `ours` against Chrome's
/// unpremultiplied `chrome`, as the bitmap pins compare.
fn against_chrome(ours: &[u8], chrome: &[u8]) -> (u8, f32, usize) {
    let pm = |p: &[u8]| {
        let f = |c: u8| ((c as u32 * p[3] as u32 + 127) / 255) as u8;
        [f(p[0]), f(p[1]), f(p[2]), p[3]]
    };
    let (mut max, mut sum, mut painted, mut off) = (0u8, 0u64, 0usize, 0usize);
    for (a, b) in ours.chunks(4).zip(chrome.chunks(4)) {
        if b[3] != 0 {
            painted += 1;
        }
        let b = pm(b);
        let d = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        max = max.max(d);
        sum += d as u64;
        off += (d > 2) as usize;
    }
    (max, sum as f32 / painted.max(1) as f32, off)
}

/// The case's canvas drawn through the bindings with the switch on: its
/// first GPU batch (the texture blank, then every draw), and the CPU
/// bitmap (premultiplied) for comparison.
fn run(script: &str, w: u32, h: u32) -> (rustkit_bindings::canvas::GpuBatch, Vec<u8>) {
    set_gpu_canvas(Some(true));
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings
        .set_document(std::rc::Rc::new(rustkit_dom::Document::parse_html("<!DOCTYPE html><html><body></body></html>").unwrap()))
        .unwrap();
    let js = format!(
        "(function () {{ var cv = document.createElement('canvas'); cv.width = {w}; cv.height = {h}; \
         document.body.appendChild(cv); var x = cv.getContext('2d'); \
         (function (x) {{ {script} }})(x); return 1; }})()"
    );
    bindings.evaluate(&js).unwrap_or_else(|e| panic!("{script}: {e:?}"));
    set_gpu_canvas(None);
    let source = bindings.canvas_source();
    let nodes = source.nodes();
    let node = *nodes.last().expect("a canvas");
    let batch = source.take_gpu_batch(node, false).expect("a GPU context");
    let (_, _, cpu) = source.pixels(node).expect("pixels");
    (batch, cpu)
}

fn check_file(r: &mut Renderer, json: &str, bounds: &serde_json::Value, failures: &mut Vec<String>, gradients_on_gpu: &mut usize) {
    let data: serde_json::Value = serde_json::from_str(json).unwrap();
    let w = data["size"][0].as_u64().unwrap() as u32;
    let h = data["size"][1].as_u64().unwrap() as u32;
    for case in data["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let script = case["script"].as_str().unwrap();
        let chrome = unhex(case["chrome_rgba"].as_str().unwrap());
        if script.contains("drawImage") || script.contains("createPattern") {
            println!("gpu-chrome {name}: not run (needs an image loader)");
            continue;
        }
        let (batch, cpu) = run(script, w, h);
        let gradient = script.contains("Gradient(");
        let on_gpu = !matches!(batch.reset, Some(GpuReset::Pixels(_)));
        if !on_gpu {
            println!("gpu-chrome {name}: cpu (the GPU does not draw it yet)");
            if gradient {
                failures.push(format!("{name}: a gradient case is not drawn by the GPU"));
            }
            continue;
        }
        *gradients_on_gpu += gradient as usize;
        assert_eq!(r.replay_canvas(KEY, &[batch]), CanvasReplay::Drawn, "{name}");
        let ours = r.read_canvas_texture(KEY).expect("texture");
        r.remove_image(KEY);
        let (gmax, gmean, goff) = against_chrome(&ours, &chrome);
        let (cmax, cmean, coff) = against_chrome(&cpu, &chrome);
        println!(
            "gpu-chrome {name}: GPU vs Chrome max {gmax} mean {gmean:.2} >2 {:.1}% | CPU vs Chrome max {cmax} mean {cmean:.2} >2 {:.1}%",
            100.0 * goff as f32 / (w * h) as f32,
            100.0 * coff as f32 / (w * h) as f32
        );
        if gmax > WRONG || gmean > MEAN {
            failures.push(format!("{name}: GPU max {gmax} mean {gmean:.2}, past the bitmap's bounds ({WRONG} / {MEAN})"));
        }
        match own_bound(bounds, name) {
            Some((bmax, bmean)) => {
                println!("gpu-chrome {name}: own bound max {bmax} mean {bmean:.2}");
                if gmax > bmax || gmean > bmean {
                    failures.push(format!("{name}: GPU max {gmax} mean {gmean:.2}, past its own bound ({bmax} / {bmean:.2})"));
                }
            }
            None => failures.push(format!("{name}: no pinned bound (canvas_raster_gpu_bounds.json)")),
        }
    }
}

#[test]
fn every_pin_the_gpu_draws_is_within_the_bitmap_bounds_of_chrome() {
    let Some((device, queue)) = gpu() else {
        eprintln!("SKIPPED canvas_gpu_chrome: no GPU adapter");
        return;
    };
    let mut r = Renderer::new(device, queue, wgpu::TextureFormat::Bgra8Unorm).expect("renderer");
    let mut failures = Vec::new();
    let mut gradients = 0;
    let bounds: serde_json::Value = serde_json::from_str(include_str!("../../../tools/parity_oracle/canvas_raster_gpu_bounds.json")).unwrap();
    assert_eq!(bounds["chrome"], "148.0.7778.216", "the bounds' Chrome is the pinned one");
    check_file(&mut r, include_str!("../../../tools/parity_oracle/canvas_raster_cases.json"), &bounds, &mut failures, &mut gradients);
    check_file(&mut r, include_str!("../../../tools/parity_oracle/canvas_raster2_cases.json"), &bounds, &mut failures, &mut gradients);
    assert!(gradients >= 5, "the gradient cases ran on the GPU: {gradients}");
    assert!(failures.is_empty(), "{failures:#?}");
}
