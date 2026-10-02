//! HiWave Smoke Harness - RustKit edition (Windows)
//!
//! This harness exercises the RustKit engine with scripted layout stress
//! tests to validate rendering stability. It uses WRY for chrome/shelf
//! and RustKit for the main content area.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use rustkit_engine::EngineBuilder;
use rustkit_viewhost::Bounds;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use tracing::{error, info};

/// Product user agent for live URLs; keep in sync with parity-capture.
const PRODUCT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15 HiWave/1.0";

#[cfg(windows)]
use windows::Win32::Foundation::HWND;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{Rect, WebViewBuilder};

/// Performance timing collector for tracking operation durations.
struct PerfTiming {
    timings: RefCell<HashMap<&'static str, Vec<Duration>>>,
}

impl PerfTiming {
    fn new() -> Self {
        Self {
            timings: RefCell::new(HashMap::new()),
        }
    }

    fn record(&self, operation: &'static str, duration: Duration) {
        self.timings
            .borrow_mut()
            .entry(operation)
            .or_insert_with(Vec::new)
            .push(duration);
    }

    fn summary(&self) -> serde_json::Value {
        let timings = self.timings.borrow();
        let mut summary = serde_json::Map::new();

        for (op, durations) in timings.iter() {
            if durations.is_empty() {
                continue;
            }

            let count = durations.len();
            let total_ms: f64 = durations.iter().map(|d| d.as_secs_f64() * 1000.0).sum();
            let avg_ms = total_ms / count as f64;
            let min_ms = durations
                .iter()
                .map(|d| d.as_secs_f64() * 1000.0)
                .fold(f64::INFINITY, f64::min);
            let max_ms = durations
                .iter()
                .map(|d| d.as_secs_f64() * 1000.0)
                .fold(f64::NEG_INFINITY, f64::max);

            summary.insert(
                op.to_string(),
                json!({
                    "count": count,
                    "total_ms": (total_ms * 100.0).round() / 100.0,
                    "avg_ms": (avg_ms * 100.0).round() / 100.0,
                    "min_ms": (min_ms * 100.0).round() / 100.0,
                    "max_ms": (max_ms * 100.0).round() / 100.0,
                }),
            );
        }

        serde_json::Value::Object(summary)
    }
}

#[derive(Debug, Clone)]
enum UserEvent {
    Layout {
        left: f64,
        right_open: bool,
        shelf: f64,
    },
    Exit,
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    // Use minimum dimensions of 1.0 to avoid scale factor assertion failures
    Rect {
        position: LogicalPosition::new(x, y).into(),
        size: LogicalSize::new(w.max(1.0), h.max(1.0)).into(),
    }
}

/// Parse command line arguments
struct Args {
    duration_ms: u64,
    dump_frame: Option<String>,
    html_file: Option<String>,
    width: u32,
    height: u32,
    perf_output: Option<String>,
    multisurface: bool,
    dump_chrome_frame: Option<String>,
    /// Live URL to load through `Engine::load_url` (overrides --html-file).
    url: Option<String>,
    /// Skip the scripted sidebar/shelf layout churn so the content view stays
    /// at the full --width x --height (implied by --url: viewing, not stress).
    static_layout: bool,
    fullscreen: bool,
}

impl Args {
    fn parse() -> Self {
        let mut args = std::env::args().skip(1).peekable();
        let mut duration_ms = 4000u64;
        let mut dump_frame = None;
        let mut html_file = None;
        let mut width = 1100u32;
        let mut height = 640u32;
        let mut perf_output = None;
        let mut multisurface = false;
        let mut dump_chrome_frame = None;
        let mut url = None;
        let mut static_layout = false;
        let mut fullscreen = false;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--duration-ms" => {
                    if let Some(val) = args.next() {
                        duration_ms = val.parse().unwrap_or(4000);
                    }
                }
                "--dump-frame" => {
                    dump_frame = args.next();
                }
                "--html-file" => {
                    html_file = args.next();
                }
                "--width" => {
                    if let Some(val) = args.next() {
                        width = val.parse().unwrap_or(1100);
                    }
                }
                "--height" => {
                    if let Some(val) = args.next() {
                        height = val.parse().unwrap_or(640);
                    }
                }
                "--perf-output" => {
                    perf_output = args.next();
                }
                "--multisurface" => {
                    multisurface = true;
                }
                "--dump-chrome-frame" => {
                    dump_chrome_frame = args.next();
                }
                "--url" => {
                    url = args.next();
                }
                "--static" => {
                    static_layout = true;
                }
                "--fullscreen" => {
                    fullscreen = true;
                }
                _ => {}
            }
        }

        Self {
            duration_ms,
            dump_frame,
            html_file,
            width,
            height,
            perf_output,
            multisurface,
            dump_chrome_frame,
            static_layout: static_layout || url.is_some(),
            url,
            fullscreen,
        }
    }
    
    /// Load HTML content from file or use default test HTML
    fn load_html_content(&self) -> String {
        if let Some(ref path) = self.html_file {
            match std::fs::read_to_string(path) {
                Ok(content) => return content,
                Err(e) => {
                    eprintln!("Warning: Failed to read HTML file {}: {}", path, e);
                }
            }
        }
        
        // Default test HTML
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>RustKit Smoke Test</title>
    <style>
        :root {
            --bg-primary: #0b1a2a;
            --text-primary: #d7e8ff;
            --accent: #4a90d9;
        }
        body {
            margin: 0;
            padding: 20px;
            background: var(--bg-primary);
            color: var(--text-primary);
            font-family: system-ui, sans-serif;
        }
        .hero {
            text-align: center;
            padding: 40px 20px;
        }
        h1 {
            font-size: 32px;
            color: var(--accent);
            margin-bottom: 16px;
        }
        p {
            font-size: 18px;
            line-height: 1.6;
        }
        .box {
            background: rgba(74, 144, 217, 0.2);
            border: 1px solid var(--accent);
            border-radius: 8px;
            padding: 20px;
            margin: 20px auto;
            max-width: 600px;
        }
    </style>
</head>
<body>
    <div class="hero">
        <h1>RustKit Engine</h1>
        <p>This content is rendered by the RustKit browser engine.</p>
    </div>
    <div class="box">
        <p>If you can read this, the engine is working!</p>
        <p>Smoke test timestamp: <span id="time">loading...</span></p>
    </div>
</body>
</html>"#.to_string()
    }
}

/// Extract HWND from a tao window on Windows
#[cfg(windows)]
fn get_hwnd_from_window(window: &tao::window::Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => {
            let hwnd_ptr = win32.hwnd.get() as isize;
            Some(HWND(hwnd_ptr as *mut std::ffi::c_void))
        }
        _ => None,
    }
}

fn spawn_scripted_flow(proxy: EventLoopProxy<UserEvent>, duration_ms: u64, static_layout: bool) {
    std::thread::spawn(move || {
        let start = Instant::now();

        // Phase 1: sidebar drag simulation. It ends with a 232px left
        // sidebar, the 220px right one open and a 120px shelf, so a
        // 1280x800 request shows content at 828x680. --static (and --url)
        // skip it so the page is viewed at the requested size.
        for i in 0..if static_layout { 0 } else { 30 } {
            let left = (i as f64) * 8.0; // 0..240
            let right_open = i % 10 >= 5;
            let shelf = if i % 2 == 0 { 0.0 } else { 120.0 };
            let _ = proxy.send_event(UserEvent::Layout {
                left,
                right_open,
                shelf,
            });
            std::thread::sleep(Duration::from_millis(30));
        }

        // Phase 2: let engine render content
        // (we avoid network dependency in the harness itself)

        // Let UI settle
        while start.elapsed() < Duration::from_millis(duration_ms) {
            std::thread::sleep(Duration::from_millis(50));
        }

        let _ = proxy.send_event(UserEvent::Exit);
    });
}

fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args = Args::parse();
    info!(
        duration_ms = args.duration_ms,
        dump_frame = ?args.dump_frame,
        html_file = ?args.html_file,
        width = args.width,
        height = args.height,
        url = ?args.url,
        fullscreen = args.fullscreen,
        "Starting HiWave Smoke Harness (RustKit)"
    );

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Size the window from the args: content is --width x --height, the
    // chrome bar sits above it.
    let chrome_height = 72.0_f64;
    let window_width = args.width as f64;
    let window_height = args.height as f64 + chrome_height;
    let mut window_builder = WindowBuilder::new()
        .with_title("HiWave Smoke Harness (RustKit)")
        .with_inner_size(tao::dpi::LogicalSize::new(window_width, window_height))
        .with_visible(true);
    if args.fullscreen {
        window_builder = window_builder
            .with_fullscreen(Some(tao::window::Fullscreen::Borderless(None)));
    }
    let window = window_builder
        .build(&event_loop)
        .expect("Failed to create window");

    // Actual size (fullscreen ignores the request), in logical pixels.
    let inner_size = window.inner_size();
    let actual_width = inner_size.width as f64 / window.scale_factor();
    let actual_height = inner_size.height as f64 / window.scale_factor();
    
    // Ensure window is visible before creating child webviews
    window.set_visible(true);

    // Chrome bar (using WRY for simple UI)
    let chrome = WebViewBuilder::new()
        .with_html(
            r#"<!doctype html><meta charset='utf-8'/>
            <body style='margin:0;background:#111;color:#fff;font:16px system-ui;display:flex;align-items:center;justify-content:center;'>
              chrome
            </body>"#,
        )
        .with_bounds(rect(0.0, 0.0, actual_width, chrome_height))
        .build_as_child(&window)
        .expect("Failed to create chrome webview");

    // Shelf (using WRY for simple UI)
    let shelf = WebViewBuilder::new()
        .with_html(
            r#"<!doctype html><meta charset='utf-8'/>
            <body style='margin:0;background:#1a0b2a;color:#f0d7ff;font:16px system-ui;display:flex;align-items:center;justify-content:center;'>
              shelf
            </body>"#,
        )
        .with_bounds(rect(0.0, actual_height, actual_width, 0.0))
        .build_as_child(&window)
        .expect("Failed to create shelf webview");

    // Performance timing collector
    let perf = PerfTiming::new();
    let perf_output = args.perf_output.clone();

    // Content area (using RustKit engine)
    // Use parity testing config to disable animations for deterministic capture
    let engine_start = Instant::now();
    // A live URL is fetched with the product's user agent, so sites serve
    // what they serve HiWave users (same UA as `parity-capture --url`).
    let mut builder = EngineBuilder::new()
        .with_config(rustkit_engine::EngineConfig::for_parity_testing());
    if args.url.is_some() {
        builder = builder.user_agent(PRODUCT_USER_AGENT);
    }
    let mut engine = builder.build().expect("Failed to create RustKit engine");
    perf.record("engine_init", engine_start.elapsed());

    // Get the HWND from the window for creating the RustKit view
    #[cfg(windows)]
    let hwnd = get_hwnd_from_window(&window).expect("Failed to get HWND from window");

    // Content fills the window below the chrome bar (the actual size matters
    // in fullscreen; otherwise it is --width x --height).
    let content_bounds = Bounds {
        x: 0,
        y: chrome_height as i32,
        width: actual_width as u32,
        height: (actual_height - chrome_height).max(0.0) as u32,
    };

    let view_start = Instant::now();
    #[cfg(windows)]
    let content_view_id = engine
        .create_view(hwnd, content_bounds)
        .expect("Failed to create RustKit content view");
    perf.record("view_create", view_start.elapsed());

    // Load content: a live URL through the browser's navigation path
    // (document, then stylesheets/fonts/images), else the HTML file/default.
    let load_start = Instant::now();
    if let Some(raw) = args.url.as_deref() {
        match url::Url::parse(raw) {
            Ok(u) => {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create tokio runtime");
                if let Err(e) = rt.block_on(engine.load_url(content_view_id, u)) {
                    error!(?e, url = raw, "Failed to load URL into RustKit view");
                }
            }
            Err(e) => error!(?e, url = raw, "Invalid --url"),
        }
    } else {
        let test_html = args.load_html_content();
        if let Err(e) = engine.load_html(content_view_id, &test_html) {
            error!(?e, "Failed to load HTML into RustKit view");
        }
    }
    perf.record("html_load", load_start.elapsed());

    // Initial render
    let render_start = Instant::now();
    if let Err(e) = engine.render_view(content_view_id) {
        error!(?e, "Failed to render RustKit view");
    }
    perf.record("render", render_start.elapsed());

    spawn_scripted_flow(proxy, args.duration_ms, args.static_layout);

    let mut last_layout = (0.0_f64, false, 0.0_f64);
    let start = Instant::now();
    let dump_frame_path = args.dump_frame.clone();
    let mut frame_dumped = false;
    let mut render_count = 0u32;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                let result = json!({
                    "status": "fail",
                    "reason": "user_closed_window",
                    "elapsed_ms": start.elapsed().as_millis()
                });
                println!("{}", result);
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                // Update RustKit view bounds on window resize
                let (left, right_open, shelf_h) = last_layout;
                let width = size.width as i32;
                let height = size.height as i32;

                let chrome_h = 72;
                let right_sidebar_width = if right_open { 220 } else { 0 };
                let left_i32 = left as i32;

                let content_w = (width - left_i32 - right_sidebar_width).max(0);
                let content_h = (height - chrome_h - shelf_h as i32).max(0);

                let bounds = Bounds {
                    x: left_i32,
                    y: chrome_h,
                    width: content_w as u32,
                    height: content_h as u32,
                };

                if let Err(e) = engine.resize_view(content_view_id, bounds) {
                    error!(?e, "Failed to resize RustKit view");
                }
            }
            Event::UserEvent(UserEvent::Layout {
                left,
                right_open,
                shelf: shelf_h,
            }) => {
                last_layout = (left, right_open, shelf_h);

                let size = window.inner_size();
                let width = size.width as f64;
                let height = size.height as f64;

                let chrome_h = 72.0;
                let right_sidebar_width = if right_open { 220.0 } else { 0.0 };

                let content_w = (width - left - right_sidebar_width).max(0.0);
                let content_h = (height - chrome_h - shelf_h).max(0.0);

                // Update WRY views
                let _ = chrome.set_bounds(rect(0.0, 0.0, width, chrome_h));
                let _ = shelf.set_bounds(rect(left, height - shelf_h, content_w, shelf_h));

                // Update RustKit content view
                let bounds = Bounds {
                    x: left as i32,
                    y: chrome_h as i32,
                    width: content_w as u32,
                    height: content_h as u32,
                };

                if let Err(e) = engine.resize_view(content_view_id, bounds) {
                    error!(?e, "Failed to resize RustKit view");
                }

                // Re-render after resize
                let render_start = Instant::now();
                if let Err(e) = engine.render_view(content_view_id) {
                    error!(?e, "Failed to render RustKit view");
                } else {
                    perf.record("render", render_start.elapsed());
                    render_count += 1;
                }
            }
            Event::UserEvent(UserEvent::Exit) => {
                // Capture frame before exit if requested
                if let Some(ref path) = dump_frame_path {
                    if !frame_dumped {
                        info!(?path, "Dumping frame to file");
                        let capture_start = Instant::now();
                        // Windows uses capture_view_screenshot with Path
                        let output_path = std::path::Path::new(path);
                        match engine.capture_view_screenshot(content_view_id, output_path) {
                            Ok(metadata) => {
                                perf.record("frame_capture", capture_start.elapsed());
                                info!(
                                    width = metadata.width,
                                    height = metadata.height,
                                    "Frame captured successfully"
                                );
                                frame_dumped = true;
                                // Note: export_layout_json is not available on Windows
                            }
                            Err(e) => {
                                error!(?e, "Failed to capture frame");
                            }
                        }
                    }
                }

                // Write perf summary if requested
                if let Some(ref perf_path) = perf_output {
                    let perf_json = json!({
                        "timings": perf.summary(),
                        "render_count": render_count,
                        "total_elapsed_ms": start.elapsed().as_millis()
                    });
                    if let Err(e) = std::fs::write(perf_path, perf_json.to_string()) {
                        error!(?e, "Failed to write perf output");
                    } else {
                        info!(?perf_path, "Perf summary written");
                    }
                }

                let (left, right_open, shelf_h) = last_layout;
                let result = json!({
                    "status": "pass",
                    "elapsed_ms": start.elapsed().as_millis(),
                    "final_layout": {
                        "left_sidebar_width": left,
                        "right_sidebar_open": right_open,
                        "shelf_height": shelf_h
                    },
                    "frame_dumped": frame_dumped,
                    "render_count": render_count,
                    "perf": perf.summary()
                });
                println!("{}", result);
                *control_flow = ControlFlow::Exit;
            }
            Event::MainEventsCleared => {
                // Periodically re-render (at least once per frame)
                let render_start = Instant::now();
                if let Err(e) = engine.render_view(content_view_id) {
                    // Only log first few errors to avoid spam
                    static RENDER_ERROR_COUNT: std::sync::atomic::AtomicU32 =
                        std::sync::atomic::AtomicU32::new(0);
                    let count = RENDER_ERROR_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if count < 3 {
                        error!(?e, "Failed to render RustKit view");
                    }
                } else {
                    perf.record("render", render_start.elapsed());
                    render_count += 1;
                }
            }
            _ => {}
        }
    });
}

