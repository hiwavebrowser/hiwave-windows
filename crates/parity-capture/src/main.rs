//! Headless frame capture tool for parity testing.
//!
//! This tool renders HTML files using RustKit's headless mode and exports:
//! - PPM frame capture
//! - Layout tree JSON
//! - Performance metrics
//!
//! Unlike hiwave-smoke, this does NOT require a display and can run in CI.

use clap::Parser;
use rustkit_engine::{EngineBuilder, EngineConfig, ScriptOutcome, ScriptRecord};
use rustkit_viewhost::Bounds;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::{error, warn};
use url::Url;

#[derive(Parser, Debug, Clone)]
#[command(name = "parity-capture")]
#[command(about = "Headless frame capture for parity testing")]
#[command(group(clap::ArgGroup::new("source").required(true).args(["html_file", "url"])))]
struct Args {
    /// Path to HTML file to render
    #[arg(long)]
    html_file: Option<String>,

    /// Live URL to load through `Engine::load_url` (document + subresources
    /// over the network), for the real-site board
    #[arg(long)]
    url: Option<String>,

    /// Hard wall-clock limit for the whole capture, in milliseconds. On
    /// expiry the process prints a `timeout` result and exits 3.
    #[arg(long, default_value = "30000")]
    timeout_ms: u64,

    /// Output path for display-list JSON (paint commands, including text runs)
    #[arg(long)]
    dump_display_list: Option<String>,

    /// Output path for the page's script log (URL mode: one record per
    /// `<script>`, plus exceptions from lifecycle listeners and timers)
    #[arg(long)]
    dump_scripts: Option<String>,

    /// Viewport width
    #[arg(long, default_value = "1280")]
    width: u32,

    /// Viewport height
    #[arg(long, default_value = "800")]
    height: u32,

    /// After loading at --width x --height, resize the view to `WxH` (through
    /// `Engine::resize_view`, as a window resize does) and capture at the new
    /// size. Compare against a fresh load at `WxH` to find layout that stays
    /// stale across a resize.
    #[arg(long, value_parser = parse_size)]
    resize_to: Option<(u32, u32)>,

    /// Output path for PPM frame
    #[arg(long)]
    dump_frame: Option<String>,

    /// Output path for layout JSON
    #[arg(long)]
    dump_layout: Option<String>,

    /// JSON string or path to JSON file defining interaction sequence (wait/click/key/resize/capture)
    #[arg(long)]
    actions: Option<String>,

    /// Directory for action capture frames
    #[arg(long)]
    actions_out_dir: Option<String>,

    /// Optional HTTP replay proxy URL (e.g. http://127.0.0.1:8989) for deterministic HAR replay.
    /// Test-only; routes outbound requests to the local replay server while preserving document origin.
    #[arg(long)]
    replay_proxy: Option<String>,

    /// Virtual timer clock horizon in milliseconds (default: 5000).
    #[arg(long)]
    timer_horizon_ms: Option<u64>,

    /// Wall-clock budget for the page's scripts in milliseconds (default: 5000, the
    /// engine's and the board's). The live app runs pages at 60000; a capture taken
    /// with another budget is not comparable with the board and must be labelled.
    #[arg(long)]
    script_budget_ms: Option<u64>,

    /// Stop a script that is still running when the script budget is spent, as the
    /// live app does (off by default: the board lets a running script finish). A
    /// capture taken with it is not comparable with the board and must be labelled.
    #[arg(long)]
    interrupt_scripts: bool,

    /// After a URL load, turn the live loop as the app does for this many
    /// milliseconds of real time (timers, script requests, relayouts) and
    /// report what the turns did as `live_stats`. Off by default; a frame
    /// taken with it is not comparable with the board.
    #[arg(long)]
    live_ms: Option<u64>,

    /// Enable verbose output
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Serialize, Deserialize)]
struct CaptureResult {
    status: String,
    html_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    url: Option<String>,
    width: u32,
    height: u32,
    frame_path: Option<String>,
    layout_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    display_list_path: Option<String>,
    layout_stats: Option<LayoutStats>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    script_stats: Option<ScriptStats>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    live_stats: Option<LiveStats>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    elapsed_ms: Option<u64>,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    captures: Option<Vec<StepCapture>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    action_results: Option<Vec<ActionResult>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ActionItem {
    #[serde(default)]
    step: Option<usize>,
    #[serde(rename = "type")]
    action_type: String,
    #[serde(default)]
    ms: Option<u64>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    x: Option<f32>,
    #[serde(default)]
    y: Option<f32>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    frame: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ActionsPayload {
    #[serde(default)]
    actions: Vec<ActionItem>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct StepCapture {
    step: usize,
    label: String,
    frame: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ActionResult {
    step: usize,
    #[serde(rename = "type")]
    action_type: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selector_used: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    elapsed_ms: u64,
}

impl CaptureResult {
    fn new(args: &Args) -> Self {
        CaptureResult {
            status: "ok".to_string(),
            html_file: args.html_file.clone(),
            url: args.url.clone(),
            width: args.width,
            height: args.height,
            frame_path: None,
            layout_path: None,
            display_list_path: None,
            layout_stats: None,
            script_stats: None,
            live_stats: None,
            elapsed_ms: None,
            error: None,
            captures: None,
            action_results: None,
        }
    }

    fn failed(mut self, status: &str, error: String) -> Self {
        self.status = status.to_string();
        self.error = Some(error);
        self
    }
}

/// Totals over a URL capture's script log.
#[derive(Serialize, Deserialize)]
struct ScriptStats {
    ran: u32,
    threw: u32,
    skipped: u32,
    fetch_failed: u32,
    over_budget: u32,
    bytes: u64,
    elapsed_ms: u64,
}

/// What the live loop did after the load (`--live-ms`).
#[derive(Serialize, Deserialize, Default)]
struct LiveStats {
    /// Real time the loop was given.
    wall_ms: u64,
    /// Of it, time spent inside turns: the window thread's time in the app.
    busy_ms: u64,
    turns: u32,
    timer_callbacks: u64,
    requests: u64,
    relayouts: u32,
    /// The page had no timer set and nothing in flight when the loop ended.
    idle_at_end: bool,
}

/// The app's pacing (`hiwave-app` `process_events`): a turn when the next
/// timer is due or a request is out, never sooner than the last turn took.
fn run_live(
    rt: &tokio::runtime::Runtime,
    engine: &mut rustkit_engine::Engine,
    view_id: rustkit_engine::EngineViewId,
    live_ms: u64,
) -> LiveStats {
    const MIN_LIVE_TURN: Duration = Duration::from_millis(4);
    const LIVE_REQUEST_POLL: Duration = Duration::from_millis(10);
    let limit = Duration::from_millis(live_ms);
    let began = Instant::now();
    let mut clock = began;
    let mut stats = LiveStats::default();
    tracing::info!(live_ms, "Live loop started");
    loop {
        let started = Instant::now();
        let elapsed_ms = started.duration_since(clock).as_millis() as u64;
        clock += Duration::from_millis(elapsed_ms);
        let turn = rt.block_on(engine.pump_live(view_id, elapsed_ms));
        stats.turns += 1;
        stats.timer_callbacks += turn.timers_ran as u64;
        stats.requests += turn.requests as u64;
        stats.relayouts += turn.relaid_out as u32;
        stats.busy_ms += started.elapsed().as_millis() as u64;
        let timer = turn.next_timer_ms.map(Duration::from_millis);
        let next = if turn.in_flight > 0 {
            Some(timer.map_or(LIVE_REQUEST_POLL, |t| t.min(LIVE_REQUEST_POLL)))
        } else {
            timer
        };
        stats.idle_at_end = next.is_none();
        let left = limit.saturating_sub(began.elapsed());
        if left.is_zero() {
            break;
        }
        // An idle page still gets its time: in the app only input wakes it.
        let wait = next.map_or(left, |wait| wait.max(started.elapsed()).max(MIN_LIVE_TURN));
        std::thread::sleep(wait.min(left));
    }
    stats.wall_ms = began.elapsed().as_millis() as u64;
    stats
}

#[derive(Serialize, Deserialize)]
struct LayoutStats {
    total_boxes: u32,
    sized: u32,
    zero_size: u32,
    positioned: u32,
    at_origin: u32,
    sizing_rate: f32,
    positioning_rate: f32,
}

fn main() {
    let args = Args::parse();

    // Initialize tracing - respect RUST_LOG if set, otherwise use defaults
    let default_filter = if args.verbose { "info" } else { "warn" };
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| default_filter.to_string());
    tracing_subscriber::fmt()
        .with_env_filter(&filter)
        .with_writer(std::io::stderr)
        .init();

    // Hard wall-clock limit. A live page can stall the engine anywhere
    // (network, layout, paint) and nothing inside the engine can be trusted
    // to give up, so the limit is enforced from outside the capture thread.
    {
        let limit = Duration::from_millis(args.timeout_ms);
        let timeout_result = CaptureResult::new(&args).failed(
            "timeout",
            format!("capture exceeded {} ms", args.timeout_ms),
        );
        std::thread::spawn(move || {
            std::thread::sleep(limit);
            println!("{}", serde_json::to_string(&timeout_result).unwrap());
            std::process::exit(3);
        });
    }

    let started = Instant::now();
    let args_clone = args.clone();
    let handler = std::thread::Builder::new()
        .name("capture-worker".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || run_capture(&args_clone))
        .expect("failed to spawn capture worker thread");
    let mut result = match handler.join() {
        Ok(r) => r,
        Err(_) => CaptureResult::new(&args).failed("error", "worker thread panicked".to_string()),
    };
    result.elapsed_ms = Some(started.elapsed().as_millis() as u64);

    // Output JSON result
    println!("{}", serde_json::to_string(&result).unwrap());

    // Exit with appropriate code
    if result.status == "ok" {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}

/// The user agent the shipping RustKit content view sends
/// (hiwave-app/src/webview_rustkit.rs). Live sites branch on it, so a URL
/// capture must present as the product does, not as a test tool.
const PRODUCT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15 HiWave/1.0";

fn run_capture(args: &Args) -> CaptureResult {
    let mut result = CaptureResult::new(args);

    // Read HTML file (URL mode fetches inside the engine instead)
    let html_content = match &args.html_file {
        Some(html_file) => match fs::read_to_string(html_file) {
            Ok(content) => Some(preprocess_html(&content, Path::new(html_file))),
            Err(e) => {
                return result.failed("error", format!("Failed to read HTML file: {}", e));
            }
        },
        None => None,
    };
    let url = match &args.url {
        Some(raw) => match Url::parse(raw) {
            Ok(u) => Some(u),
            Err(e) => return result.failed("error", format!("Invalid URL: {}", e)),
        },
        None => None,
    };

    // Create engine with parity testing config (animations disabled).
    // Fixture mode keeps its historical test-tool UA; URL mode sends the
    // product's. URL mode runs the page's scripts, as the browser does;
    // fixture mode does not (the campaign fixtures are static pages).
    let user_agent = if url.is_some() {
        PRODUCT_USER_AGENT
    } else {
        "ParityCapture/1.0"
    };

    let replay_proxy = match &args.replay_proxy {
        Some(raw) => match Url::parse(raw) {
            Ok(u) => Some(u),
            Err(e) => return result.failed("error", format!("Invalid replay proxy URL: {}", e)),
        },
        None => None,
    };

    let engine_result = EngineBuilder::new()
        .with_config(capture_config(&args, replay_proxy))
        .user_agent(user_agent)
        .javascript_enabled(url.is_some() || args.actions.is_some())
        .build();

    let mut engine = match engine_result {
        Ok(e) => e,
        Err(e) => return result.failed("error", format!("Failed to create engine: {:?}", e)),
    };

    // Create headless view
    let bounds = Bounds {
        x: 0,
        y: 0,
        width: args.width,
        height: args.height,
    };

    let view_id = match engine.create_headless_view(bounds) {
        Ok(id) => id,
        Err(e) => {
            return result.failed("error", format!("Failed to create headless view: {:?}", e));
        }
    };

    // Load: either the fixture HTML, or the live URL through the same
    // navigation path the browser uses (document fetch, then stylesheets,
    // fonts and images via load_subresources).
    if let Some(url) = url {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return result.failed("error", format!("Failed to create runtime: {}", e)),
        };
        if let Err(e) = rt.block_on(engine.load_url(view_id, url)) {
            return result.failed("error", format!("Failed to load URL: {:?}", e));
        }
        // The engine shows a server's error page as the page, which is what
        // the app's user should see. A capture is of the site, not of its
        // error page: it fails as it did when the engine refused the
        // response, with the same message first, so no board counts a 403
        // as a load.
        if let Some(status) = engine
            .http_status(view_id)
            .filter(|s| !(200..300).contains(s))
        {
            return result.failed(
                "error",
                format!("Failed to load URL: NavigationError(\"HTTP error\") (HTTP {status}; the engine rendered its body)"),
            );
        }
        if let Some(log) = engine.script_log(view_id) {
            result.script_stats = Some(script_stats(log));
            if let Some(ref path) = args.dump_scripts {
                if let Err(e) = fs::write(path, script_log_json(log).to_string()) {
                    error!("Failed to write script log: {:?}", e);
                }
            }
        }
        if let Some(live_ms) = args.live_ms {
            result.live_stats = Some(run_live(&rt, &mut engine, view_id, live_ms));
        }
    } else if let Some(html) = html_content {
        if let Err(e) = engine.load_html(view_id, &html) {
            return result.failed("error", format!("Failed to load HTML: {:?}", e));
        }
    }

    if let Some((width, height)) = args.resize_to {
        let bounds = Bounds {
            x: 0,
            y: 0,
            width,
            height,
        };
        if let Err(e) = engine.resize_view(view_id, bounds) {
            return result.failed("error", format!("Failed to resize view: {:?}", e));
        }
        result.width = width;
        result.height = height;
    }

    if let Some(ref actions_raw) = args.actions {
        let actions = match parse_actions(actions_raw) {
            Ok(acts) => acts,
            Err(e) => {
                let _ = engine.destroy_view(view_id);
                return result.failed("error", e);
            }
        };
        execute_actions(
            &mut engine,
            view_id,
            &actions,
            args.actions_out_dir.as_deref(),
            &mut result,
        );
    }

    // Render final view
    if let Err(e) = engine.render_view(view_id) {
        return result.failed("error", format!("Failed to render: {:?}", e));
    }

    // Capture frame if requested
    if let Some(ref path) = args.dump_frame {
        if let Err(e) = engine.capture_frame(view_id, path) {
            error!("Failed to capture frame: {:?}", e);
        } else {
            result.frame_path = Some(path.clone());
        }
    }

    // Export layout if requested
    if let Some(ref path) = args.dump_layout {
        match engine.export_layout_json(view_id, path) {
            Ok(()) => {
                result.layout_path = Some(path.clone());
                // Read back the file to analyze
                match fs::read_to_string(path) {
                    Ok(layout_json) => result.layout_stats = analyze_layout_json(&layout_json),
                    Err(e) => error!("Failed to read layout file: {:?}", e),
                }
            }
            Err(e) => error!("Failed to export layout: {:?}", e),
        }
    }

    // Export display list if requested
    if let Some(ref path) = args.dump_display_list {
        match engine.export_display_list_json(view_id, path) {
            Ok(()) => result.display_list_path = Some(path.clone()),
            Err(e) => error!("Failed to export display list: {:?}", e),
        }
    }

    // Clean up
    let _ = engine.destroy_view(view_id);

    result
}

fn parse_actions(raw: &str) -> Result<Vec<ActionItem>, String> {
    let trimmed = raw.trim();
    let content = if trimmed.starts_with('[') || trimmed.starts_with('{') {
        trimmed.to_string()
    } else {
        fs::read_to_string(trimmed).map_err(|e| format!("failed to read actions file: {e}"))?
    };
    let content = content.trim();
    if content.starts_with('[') {
        serde_json::from_str::<Vec<ActionItem>>(content)
            .map_err(|e| format!("failed to parse actions JSON array: {e}"))
    } else {
        serde_json::from_str::<ActionsPayload>(content)
            .map(|p| p.actions)
            .map_err(|e| format!("failed to parse actions JSON object: {e}"))
    }
}

fn parse_point(eval_res: &str) -> Option<(f32, f32)> {
    let idx = eval_res.find("point:")?;
    let rest = &eval_res[idx + 6..];
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() < 2 {
        return None;
    }
    let x_part = parts[0].trim_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '-');
    let y_part = parts[1].trim_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '-');
    let x = x_part.parse::<f32>().ok()?;
    let y = y_part.parse::<f32>().ok()?;
    Some((x, y))
}

fn execute_actions(
    engine: &mut rustkit_engine::Engine,
    view_id: rustkit_engine::EngineViewId,
    actions: &[ActionItem],
    actions_out_dir: Option<&str>,
    result: &mut CaptureResult,
) {
    let mut captures = Vec::new();
    let mut action_results = Vec::new();

    for (i, a) in actions.iter().enumerate() {
        let step = a.step.unwrap_or(i);
        let start = Instant::now();
        let mut a_res = ActionResult {
            step,
            action_type: a.action_type.clone(),
            status: "ok".to_string(),
            error: None,
            frame: None,
            label: a.label.clone(),
            selector_used: None,
            method: None,
            elapsed_ms: 0,
        };

        match a.action_type.as_str() {
            "wait" => {
                if let Some(ref sel) = a.selector {
                    let timeout = Duration::from_millis(a.timeout_ms.or(a.ms).unwrap_or(2000));
                    let wait_start = Instant::now();
                    let sel_json = serde_json::to_string(sel).unwrap_or_default();
                    let check_js = format!(
                        "Boolean(document.querySelector({})) ? 'found' : 'missing'",
                        sel_json
                    );
                    let mut found = false;
                    while wait_start.elapsed() < timeout {
                        if let Ok(eval_res) = engine.execute_script(view_id, &check_js) {
                            if eval_res.contains("found") && !eval_res.contains("missing") {
                                found = true;
                                break;
                            }
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    if !found {
                        a_res.status = "timeout".to_string();
                        a_res.error = Some(format!("timed out waiting for selector {}", sel));
                    }
                } else {
                    let ms = a.ms.unwrap_or(500);
                    std::thread::sleep(Duration::from_millis(ms));
                }
            }
            "click" => {
                if let Some(ref sel) = a.selector {
                    let selectors: Vec<&str> = sel.split(',').map(|s| s.trim()).collect();
                    let mut clicked = false;
                    for s in selectors {
                        let s_json = serde_json::to_string(s).unwrap_or_default();
                        let query_js = format!(
                            r#"(function() {{
                                try {{
                                    var el = document.querySelector({0});
                                    if (!el) return 'missing';
                                    if (typeof el.getBoundingClientRect === 'function') {{
                                        var r = el.getBoundingClientRect();
                                        if (r && r.width > 0 && r.height > 0) {{
                                            var cx = r.left + r.width / 2.0;
                                            var cy = r.top + r.height / 2.0;
                                            return 'point:' + cx + ':' + cy;
                                        }}
                                    }}
                                    if (typeof el.focus === 'function') {{
                                        try {{ el.focus(); }} catch (e) {{}}
                                    }}
                                    if (typeof el.click === 'function') {{
                                        try {{ el.click(); }} catch (e) {{}}
                                    }} else {{
                                        try {{ el.dispatchEvent(new Event('click', {{ bubbles: true, cancelable: true }})); }} catch (e) {{}}
                                    }}
                                    return 'fallback_clicked';
                                }} catch (e) {{
                                    return 'error:' + e;
                                }}
                            }})()"#,
                            s_json
                        );
                        if let Ok(eval_res) = engine.execute_script(view_id, &query_js) {
                            if let Some((cx, cy)) = parse_point(&eval_res) {
                                engine.mouse_move_at_point(view_id, cx, cy);
                                let _ = engine.mouse_down_at_point(view_id, cx, cy);
                                let _ = engine.click_at_point(view_id, cx, cy);
                                let _ = engine.relayout(view_id);
                                clicked = true;
                                a_res.selector_used = Some(s.to_string());
                                a_res.method = Some(format!("engine_point({:.1}, {:.1})", cx, cy));
                                break;
                            }
                            if eval_res.contains("fallback_clicked") {
                                let _ = engine.relayout(view_id);
                                clicked = true;
                                a_res.selector_used = Some(s.to_string());
                                a_res.method = Some("synthetic_fallback".to_string());
                                if a_res.label.is_none() {
                                    a_res.label = Some("fallback".to_string());
                                }
                                break;
                            }
                        }
                    }
                    if !clicked {
                        a_res.status = "selector_not_found".to_string();
                        a_res.error =
                            Some(format!("no matching element found for selector {}", sel));
                    }
                } else if let (Some(x), Some(y)) = (a.x, a.y) {
                    engine.mouse_move_at_point(view_id, x, y);
                    let _ = engine.mouse_down_at_point(view_id, x, y);
                    let _ = engine.click_at_point(view_id, x, y);
                    let _ = engine.relayout(view_id);
                    a_res.method = Some(format!("engine_point({:.1}, {:.1})", x, y));
                } else {
                    a_res.status = "error".to_string();
                    a_res.error = Some("click action requires selector or (x, y)".to_string());
                }
            }
            "key" => {
                if let Some(ref sel) = a.selector {
                    let sel_json = serde_json::to_string(sel).unwrap_or_default();
                    let focus_js = format!(
                        "var el = document.querySelector({}); if (el && typeof el.focus === 'function') el.focus();",
                        sel_json
                    );
                    let _ = engine.execute_script(view_id, &focus_js);
                }
                if let Some(ref text) = a.text {
                    let text_json = serde_json::to_string(text).unwrap_or_default();
                    let key_js = format!(
                        r#"(function() {{
                            var el = document.activeElement;
                            if (el && 'value' in el) {{
                                el.value = (el.value || '') + {0};
                                el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                                el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                            }}
                        }})()"#,
                        text_json
                    );
                    let _ = engine.execute_script(view_id, &key_js);
                    let _ = engine.relayout(view_id);
                } else if let Some(ref key) = a.key {
                    let key_code = match key.as_str() {
                        "Enter" => 13,
                        "Escape" => 27,
                        "Backspace" => 8,
                        "Tab" => 9,
                        _ => 0,
                    };
                    let _ = engine.handle_text_key(view_id, key_code, key, false, false, false);
                    let key_json = serde_json::to_string(key).unwrap_or_default();
                    let event_js = format!(
                        r#"(function() {{
                            var el = document.activeElement || document.body;
                            if (el) {{
                                el.dispatchEvent(new KeyboardEvent('keydown', {{ key: {0}, bubbles: true }}));
                                el.dispatchEvent(new KeyboardEvent('keyup', {{ key: {0}, bubbles: true }}));
                            }}
                        }})()"#,
                        key_json
                    );
                    let _ = engine.execute_script(view_id, &event_js);
                    let _ = engine.relayout(view_id);
                }
            }
            "resize" => {
                if let (Some(w), Some(h)) = (a.width, a.height) {
                    let bounds = Bounds {
                        x: 0,
                        y: 0,
                        width: w,
                        height: h,
                    };
                    if let Err(e) = engine.resize_view(view_id, bounds) {
                        a_res.status = "error".to_string();
                        a_res.error = Some(format!("resize failed: {:?}", e));
                    } else {
                        result.width = w;
                        result.height = h;
                        let _ = engine.relayout(view_id);
                    }
                } else {
                    a_res.status = "error".to_string();
                    a_res.error = Some("resize requires width and height".to_string());
                }
            }
            "capture" => {
                if let Some(ref frame) = a.frame {
                    let frame_path = match actions_out_dir {
                        Some(out_dir) => {
                            Path::new(out_dir).join(frame).to_string_lossy().to_string()
                        }
                        None => frame.clone(),
                    };
                    if let Some(parent) = Path::new(&frame_path).parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = engine.render_view(view_id);
                    if let Err(e) = engine.capture_frame(view_id, &frame_path) {
                        a_res.status = "error".to_string();
                        a_res.error = Some(format!("capture_frame failed: {:?}", e));
                    } else {
                        let label = a.label.clone().unwrap_or_else(|| format!("step_{}", step));
                        captures.push(StepCapture {
                            step,
                            label: label.clone(),
                            frame: frame_path.clone(),
                        });
                        a_res.frame = Some(frame_path);
                        a_res.label = Some(label);
                    }
                } else {
                    a_res.status = "error".to_string();
                    a_res.error = Some("capture requires frame path".to_string());
                }
            }
            other => {
                a_res.status = "error".to_string();
                a_res.error = Some(format!("unknown action type: {}", other));
            }
        }

        a_res.elapsed_ms = start.elapsed().as_millis() as u64;
        action_results.push(a_res);
    }

    result.captures = Some(captures);
    result.action_results = Some(action_results);
}

/// `WxH`, e.g. `1024x768`.
fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once('x')
        .ok_or_else(|| format!("expected WxH, got {s:?}"))?;
    let w = w.parse().map_err(|e| format!("width {w:?}: {e}"))?;
    let h = h.parse().map_err(|e| format!("height {h:?}: {e}"))?;
    Ok((w, h))
}

fn script_stats(log: &[ScriptRecord]) -> ScriptStats {
    let mut stats = ScriptStats {
        ran: 0,
        threw: 0,
        skipped: 0,
        fetch_failed: 0,
        over_budget: 0,
        bytes: 0,
        elapsed_ms: 0,
    };
    for record in log {
        match record.outcome {
            ScriptOutcome::Ran => stats.ran += 1,
            ScriptOutcome::Threw(_) => stats.threw += 1,
            ScriptOutcome::Skipped(_) => stats.skipped += 1,
            ScriptOutcome::FetchFailed(_) => stats.fetch_failed += 1,
            ScriptOutcome::OverBudget => stats.over_budget += 1,
        }
        stats.bytes += record.bytes as u64;
        stats.elapsed_ms += record.elapsed_ms;
    }
    stats
}

fn script_log_json(log: &[ScriptRecord]) -> serde_json::Value {
    let records: Vec<_> = log
        .iter()
        .map(|r| {
            let (outcome, detail) = match &r.outcome {
                ScriptOutcome::Ran => ("ran", None),
                ScriptOutcome::Threw(m) => ("threw", Some(m.clone())),
                ScriptOutcome::Skipped(why) => ("skipped", Some(why.to_string())),
                ScriptOutcome::FetchFailed(m) => ("fetch_failed", Some(m.clone())),
                ScriptOutcome::OverBudget => ("over_budget", None),
            };
            serde_json::json!({
                "source": r.source,
                "bytes": r.bytes,
                "elapsed_ms": r.elapsed_ms,
                "outcome": outcome,
                "detail": detail,
            })
        })
        .collect();
    serde_json::json!({ "scripts": records })
}

/// Mirror the Chrome capture pipeline's CSS inputs for a file loaded via
/// `Engine::load_html`, which uses a synthetic about:blank base URL and never
/// fetches subresources:
///
/// 1. Inline every `<link rel="stylesheet" href="...">` whose href resolves to
///    a file relative to the HTML file's directory (Chrome loads these
///    natively over file://).
/// 2. For micro-suite fixtures, inject `baselines/common/parity-reset.css` as
///    the FIRST style in `<head>` — Chrome's capture injects it via an init
///    script before the fixture's own styles, so fixture rules win ties there
///    and must win ties here too.
fn preprocess_html(html: &str, html_path: &Path) -> String {
    let base_dir = html_path.parent().unwrap_or_else(|| Path::new("."));
    let mut out = inline_stylesheet_links(html, base_dir);

    if is_micro_suite_path(html_path) {
        if let Some(reset_css) = read_repo_parity_reset(html_path) {
            out = inject_style_first_in_head(&out, "data-parity-reset=\"1\"", &reset_css);
        } else {
            warn!("micro-suite fixture but baselines/common/parity-reset.css not found");
        }
    }

    out
}

fn is_micro_suite_path(html_path: &Path) -> bool {
    // Canonicalize first: the separator-delimited patterns below require a
    // LEADING separator, so a repo-relative invocation ("websuite/micro/x")
    // never matched and the reset was silently skipped — while CI passes
    // absolute paths and always matched. Local captures therefore rendered
    // micro cases WITHOUT the reset Chrome's oracle applies (line-height
    // 1.5 vs metrics-normal, ~5px per line), an invisible capture-
    // environment asymmetry between every local board and CI.
    let canon = html_path
        .canonicalize()
        .unwrap_or_else(|_| html_path.to_path_buf());
    let p = canon.to_string_lossy();
    p.contains("/websuite/micro/") || p.contains("\\websuite\\micro\\")
}

/// Walk up from the HTML file to the repo root (the directory containing
/// `baselines/common/parity-reset.css`) and read the reset, matching
/// deterministic.mjs's RESET_CSS_PATH.
fn read_repo_parity_reset(html_path: &Path) -> Option<String> {
    let mut dir = html_path.parent()?;
    loop {
        let candidate = dir.join("baselines/common/parity-reset.css");
        if candidate.is_file() {
            return fs::read_to_string(candidate).ok();
        }
        dir = dir.parent()?;
    }
}

/// Replace `<link rel="stylesheet" href="...">` tags with inline `<style>`
/// blocks holding the referenced file's contents, preserving document order
/// so the cascade is unchanged. Absolute (scheme://) hrefs and unreadable
/// files keep the original tag and produce a warning.
fn inline_stylesheet_links(html: &str, base_dir: &Path) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;

    while let Some(rel_start) = lower[pos..].find("<link") {
        let tag_start = pos + rel_start;
        let Some(rel_end) = lower[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + rel_end + 1;
        let tag = &html[tag_start..tag_end];

        out.push_str(&html[pos..tag_start]);
        match stylesheet_replacement_for_link_tag(tag, base_dir) {
            Some(style_block) => out.push_str(&style_block),
            None => out.push_str(tag),
        }
        pos = tag_end;
    }
    out.push_str(&html[pos..]);
    out
}

/// If the tag is a resolvable relative stylesheet link, build its inline
/// `<style>` replacement; otherwise None (keep the tag as-is).
fn stylesheet_replacement_for_link_tag(tag: &str, base_dir: &Path) -> Option<String> {
    let rel = extract_attr(tag, "rel")?;
    if !rel
        .split_ascii_whitespace()
        .any(|t| t.eq_ignore_ascii_case("stylesheet"))
    {
        return None;
    }
    let href = extract_attr(tag, "href")?;
    if href.contains("://") || href.starts_with("//") {
        warn!(href, "leaving non-local stylesheet link unresolved");
        return None;
    }

    let css_path = base_dir.join(&href);
    let css = match fs::read_to_string(&css_path) {
        Ok(css) => css,
        Err(e) => {
            warn!(href, ?css_path, ?e, "failed to read linked stylesheet");
            return None;
        }
    };
    if css.to_ascii_lowercase().contains("</style") {
        warn!(href, "stylesheet contains '</style' — cannot inline safely");
        return None;
    }

    let media = extract_attr(tag, "media").unwrap_or_default();
    let needs_media_wrap = !media.is_empty() && !media.eq_ignore_ascii_case("all");
    let body = if needs_media_wrap {
        format!("@media {} {{\n{}\n}}", media, css)
    } else {
        css
    };
    Some(format!(
        "<style data-inlined-href=\"{}\">\n{}\n</style>",
        href, body
    ))
}

/// Crude attribute extraction: name="value" or name='value' (fixtures are
/// controlled inputs; unquoted values are not used there).
fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{}=", name);
    let mut search_from = 0;
    while let Some(found) = lower[search_from..].find(&needle) {
        let idx = search_from + found;
        // Must be preceded by whitespace to be an attribute name boundary.
        let boundary_ok = idx > 0
            && lower[..idx]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_whitespace());
        let val_start = idx + needle.len();
        if boundary_ok {
            let rest = &tag[val_start..];
            let mut chars = rest.chars();
            return match chars.next() {
                Some(q @ ('"' | '\'')) => {
                    let inner = &rest[1..];
                    inner.find(q).map(|end| inner[..end].to_string())
                }
                _ => None,
            };
        }
        search_from = val_start;
    }
    None
}

/// Insert a `<style {attrs}>` block as the first child of `<head>`, falling
/// back to just after `<html...>` or the start of the document.
fn inject_style_first_in_head(html: &str, attrs: &str, css: &str) -> String {
    let style_block = format!("<style {}>\n{}\n</style>", attrs, css);
    let lower = html.to_ascii_lowercase();

    let insert_at = ["<head", "<html"].iter().find_map(|open| {
        lower
            .find(open)
            .and_then(|start| lower[start..].find('>').map(|end| start + end + 1))
    });

    match insert_at {
        Some(at) => format!("{}{}{}", &html[..at], style_block, &html[at..]),
        None => format!("{}{}", style_block, html),
    }
}

fn analyze_layout_json(json_str: &str) -> Option<LayoutStats> {
    let data: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let mut stats = LayoutStats {
        total_boxes: 0,
        sized: 0,
        zero_size: 0,
        positioned: 0,
        at_origin: 0,
        sizing_rate: 0.0,
        positioning_rate: 0.0,
    };

    fn walk(node: &serde_json::Value, stats: &mut LayoutStats) {
        if let Some(rect) = node.get("content_rect").or(node.get("rect")) {
            let x = rect.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = rect.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let w = rect.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let h = rect.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);

            stats.total_boxes += 1;

            if x != 0.0 || y != 0.0 {
                stats.positioned += 1;
            } else {
                stats.at_origin += 1;
            }

            if w > 0.0 && h > 0.0 {
                stats.sized += 1;
            } else {
                stats.zero_size += 1;
            }
        }

        if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
            for child in children {
                walk(child, stats);
            }
        }
    }

    if let Some(root) = data.get("root") {
        walk(root, &mut stats);
    }

    let total = stats.total_boxes.max(1) as f32;
    stats.sizing_rate = stats.sized as f32 / total;
    stats.positioning_rate = stats.positioned as f32 / total;

    Some(stats)
}

/// The engine configuration a capture runs with: the parity defaults, plus
/// whatever the command line overrides.
fn capture_config(args: &Args, replay_proxy: Option<Url>) -> EngineConfig {
    let mut config = EngineConfig::for_parity_testing();
    if let Some(horizon) = args.timer_horizon_ms {
        config.timer_horizon_ms = horizon;
    }
    if let Some(budget) = args.script_budget_ms {
        config.script_budget_ms = budget;
    }
    config.interrupt_scripts_at_budget = args.interrupt_scripts;
    config.replay_proxy = replay_proxy;
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn parsed(extra: &[&str]) -> Args {
        let mut argv = vec!["parity-capture", "--url", "https://example.test/"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).expect("the command line parses")
    }

    // The live loop is the app's, not the board's: a capture turns it only
    // when asked.
    #[test]
    fn the_live_loop_is_turned_only_when_asked() {
        assert_eq!(parsed(&[]).live_ms, None);
        assert_eq!(parsed(&["--live-ms", "20000"]).live_ms, Some(20_000));
    }

    // The board is measured at the engine's 5 s script budget, and the live
    // app runs pages at 60 s (#573). `--script-budget-ms` lets one labelled
    // run be taken at the app's budget; without the flag nothing changes.
    #[test]
    fn the_script_budget_is_the_engines_default_without_the_flag() {
        let config = capture_config(&parsed(&[]), None);
        assert_eq!(config.script_budget_ms, 5_000);
        assert_eq!(
            config.script_budget_ms,
            EngineConfig::for_parity_testing().script_budget_ms
        );
        assert_eq!(
            config.timer_horizon_ms,
            EngineConfig::for_parity_testing().timer_horizon_ms
        );
    }

    #[test]
    fn a_running_script_is_stopped_at_the_budget_only_with_the_flag() {
        assert!(!capture_config(&parsed(&[]), None).interrupt_scripts_at_budget);
        assert!(!capture_config(&parsed(&["--script-budget-ms", "60000"]), None).interrupt_scripts_at_budget);
        let config = capture_config(&parsed(&["--interrupt-scripts"]), None);
        assert!(config.interrupt_scripts_at_budget);
        assert_eq!(config.script_budget_ms, 5_000);
    }

    #[test]
    fn the_script_budget_flag_sets_the_engines_script_budget_and_nothing_else() {
        let config = capture_config(&parsed(&["--script-budget-ms", "60000"]), None);
        assert_eq!(config.script_budget_ms, 60_000);
        assert_eq!(
            config.timer_horizon_ms,
            EngineConfig::for_parity_testing().timer_horizon_ms
        );
    }

    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn inlines_relative_stylesheet_link_in_place() {
        let dir = std::env::temp_dir().join("pc-test-inline");
        let _ = fs::remove_dir_all(&dir);
        write_file(&dir, "common/reset.css", "h1 { color: red; }");
        let html = r#"<html><head><link rel="stylesheet" href="common/reset.css"><style>h1{color:blue}</style></head></html>"#;

        let out = inline_stylesheet_links(html, &dir);

        assert!(!out.contains("<link"), "link tag should be replaced");
        assert!(out.contains("h1 { color: red; }"));
        // Order preserved: inlined sheet before the fixture's own <style>.
        let inlined = out.find("color: red").unwrap();
        let fixture = out.find("color:blue").unwrap();
        assert!(inlined < fixture);
    }

    #[test]
    fn leaves_remote_and_missing_links_alone() {
        let dir = std::env::temp_dir().join("pc-test-remote");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let html = concat!(
            r#"<link rel="stylesheet" href="https://example.com/a.css">"#,
            r#"<link rel="stylesheet" href="missing.css">"#,
            r#"<link rel="icon" href="fav.ico">"#,
        );

        let out = inline_stylesheet_links(html, &dir);
        assert_eq!(out, html);
    }

    #[test]
    fn wraps_media_scoped_links() {
        let dir = std::env::temp_dir().join("pc-test-media");
        let _ = fs::remove_dir_all(&dir);
        write_file(&dir, "print.css", "body { display: none; }");
        let html = r#"<link rel="stylesheet" href="print.css" media="print">"#;

        let out = inline_stylesheet_links(html, &dir);
        assert!(out.contains("@media print {"));
    }

    #[test]
    fn injects_reset_first_in_head() {
        let html = "<html><head><style>b{}</style></head></html>";
        let out = inject_style_first_in_head(html, "data-parity-reset=\"1\"", "x{}");
        let reset = out.find("data-parity-reset").unwrap();
        let fixture = out.find("<style>b{}").unwrap();
        assert!(reset < fixture);
    }

    #[test]
    fn micro_suite_detection_accepts_relative_paths() {
        // The predicate must not depend on how the caller spelled the path:
        // CI passes absolute, humans pass relative, and the reset injection
        // silently diverging between them is a capture-environment asymmetry
        // (see the canonicalize comment on is_micro_suite_path).
        // Canonicalization needs a real file, so use one from the tree.
        let real = Path::new("websuite/micro/gradients/index.html");
        if real.exists() {
            assert!(
                is_micro_suite_path(real),
                "relative micro path must be detected"
            );
        }
        assert!(!is_micro_suite_path(Path::new(
            "websuite/cases/x/index.html"
        )));
    }

    #[test]
    fn micro_suite_detection_matches_deterministic_mjs() {
        assert!(is_micro_suite_path(Path::new(
            "/repo/websuite/micro/bg-solid/index.html"
        )));
        assert!(!is_micro_suite_path(Path::new(
            "/repo/websuite/pages/blog/index.html"
        )));
    }

    #[test]
    fn parse_point_extracts_coordinates_from_js_string() {
        assert_eq!(
            parse_point("String(\"point:100:200\")"),
            Some((100.0, 200.0))
        );
        assert_eq!(parse_point("point:12.5:30.25"), Some((12.5, 30.25)));
        assert_eq!(parse_point("point:-10.0:40.5"), Some((-10.0, 40.5)));
        assert_eq!(parse_point("String(\"fallback_clicked\")"), None);
        assert_eq!(parse_point("missing"), None);
    }
}
