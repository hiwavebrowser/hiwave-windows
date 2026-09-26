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

#[derive(Parser, Debug)]
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

    /// Output path for PPM frame
    #[arg(long)]
    dump_frame: Option<String>,

    /// Output path for layout JSON
    #[arg(long)]
    dump_layout: Option<String>,

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
    elapsed_ms: Option<u64>,
    error: Option<String>,
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
            elapsed_ms: None,
            error: None,
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
    let mut result = run_capture(&args);
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
    let engine_result = EngineBuilder::new()
        .with_config(EngineConfig::for_parity_testing())
        .user_agent(user_agent)
        .javascript_enabled(url.is_some())
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
        if let Some(log) = engine.script_log(view_id) {
            result.script_stats = Some(script_stats(log));
            if let Some(ref path) = args.dump_scripts {
                if let Err(e) = fs::write(path, script_log_json(log).to_string()) {
                    error!("Failed to write script log: {:?}", e);
                }
            }
        }
    } else if let Some(html) = html_content {
        if let Err(e) = engine.load_html(view_id, &html) {
            return result.failed("error", format!("Failed to load HTML: {:?}", e));
        }
    }

    // Render
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
        lower.find(open).and_then(|start| {
            lower[start..].find('>').map(|end| start + end + 1)
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
            assert!(is_micro_suite_path(real),
                "relative micro path must be detected");
        }
        assert!(!is_micro_suite_path(Path::new("websuite/cases/x/index.html")));
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
}

