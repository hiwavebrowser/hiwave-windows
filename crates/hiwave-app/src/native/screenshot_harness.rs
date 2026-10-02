//! Screenshot test harness for RustKit browser.
//!
//! Provides both GPU readback and OS window capture for render testing.

use rustkit_viewhost::screenshot as os_screenshot;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::HWND;

/// Configuration for screenshot test mode.
#[derive(Debug, Clone)]
pub struct ScreenshotConfig {
    /// Output directory for screenshots
    pub output_dir: PathBuf,
    /// Test scene to render (e.g., "text_basic", "solid_colors")
    pub scene: Option<String>,
    /// URL to load (if scene is not specified)
    pub url: Option<String>,
    /// Number of frames to wait before capture
    pub wait_frames: u32,
    /// Enable GPU readback capture
    pub gpu_capture: bool,
    /// Enable OS window capture
    pub os_capture: bool,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from(".ai/artifacts/screenshots"),
            scene: None,
            url: None,
            wait_frames: 3,
            gpu_capture: true,
            os_capture: true,
        }
    }
}

/// Result of a screenshot capture.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptureResult {
    /// Path to GPU readback PNG (if captured)
    pub gpu_path: Option<PathBuf>,
    /// Path to OS window capture PNG (if captured)  
    pub os_path: Option<PathBuf>,
    /// Metadata JSON path
    pub metadata_path: PathBuf,
    /// Width of captured image
    pub width: u32,
    /// Height of captured image
    pub height: u32,
    /// Number of frames rendered
    pub frames_rendered: u32,
    /// Scene name
    pub scene: String,
}

/// Screenshot metadata for verification.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptureMetadata {
    pub timestamp: String,
    pub scene: String,
    pub width: u32,
    pub height: u32,
    pub wait_frames: u32,
    pub gpu_captures: Vec<GpuCaptureInfo>,
    pub os_captures: Vec<OsCaptureInfo>,
    pub render_stats: Option<RenderStatsInfo>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GpuCaptureInfo {
    pub view: String,
    pub path: String,
    pub adapter: String,
    pub format: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OsCaptureInfo {
    pub view: String,
    pub path: String,
    pub capture_method: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenderStatsInfo {
    pub color_vertex_count: usize,
    pub texture_vertex_count: usize,
    pub color_index_count: usize,
    pub texture_index_count: usize,
}

/// Parse command line arguments for screenshot mode.
pub fn parse_screenshot_args() -> Option<ScreenshotConfig> {
    let args: Vec<String> = std::env::args().collect();
    
    let mut config = ScreenshotConfig::default();
    let mut is_screenshot_mode = false;
    
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--render-test" | "--screenshot-mode" => {
                is_screenshot_mode = true;
            }
            "--screenshot-scene" | "--render-test-scene" => {
                i += 1;
                if i < args.len() {
                    config.scene = Some(args[i].clone());
                }
            }
            "--screenshot-url" | "--render-test-url" => {
                i += 1;
                if i < args.len() {
                    config.url = Some(args[i].clone());
                }
            }
            "--screenshot-out" | "--screenshot-output" => {
                i += 1;
                if i < args.len() {
                    config.output_dir = PathBuf::from(&args[i]);
                }
            }
            "--screenshot-frames" => {
                i += 1;
                if i < args.len() {
                    config.wait_frames = args[i].parse().unwrap_or(3);
                }
            }
            "--no-gpu-capture" => {
                config.gpu_capture = false;
            }
            "--no-os-capture" => {
                config.os_capture = false;
            }
            _ => {}
        }
        i += 1;
    }
    
    if is_screenshot_mode {
        Some(config)
    } else {
        None
    }
}

/// Capture OS window screenshot.
pub fn capture_os_window(
    hwnd: HWND,
    output_path: &Path,
) -> Result<(u32, u32), String> {
    os_screenshot::capture_hwnd_to_png(hwnd, output_path)
        .map_err(|e| format!("OS capture failed: {}", e))
}

/// Get test scene HTML for built-in test scenes.
pub fn get_test_scene_html(scene: &str) -> Option<&'static str> {
    match scene {
        "solid_colors" => Some(SCENE_SOLID_COLORS),
        "text_basic" => Some(SCENE_TEXT_BASIC),
        "text_glyph_atlas" => Some(SCENE_TEXT_GLYPH_ATLAS),
        "alpha_blend" => Some(SCENE_ALPHA_BLEND),
        "borders" => Some(SCENE_BORDERS),
        "clip_stack" => Some(SCENE_CLIP_STACK),
        "scrolling_smoke" => Some(SCENE_SCROLLING_SMOKE),
        _ => None,
    }
}

/// List all available test scenes.
pub fn list_test_scenes() -> &'static [&'static str] {
    &[
        "solid_colors",
        "text_basic", 
        "text_glyph_atlas",
        "alpha_blend",
        "borders",
        "clip_stack",
        "scrolling_smoke",
    ]
}

/// Generate ISO8601 timestamp.
fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    utc_timestamp_from_secs(secs)
}

/// Render seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// The previous version counted 365-day years and 30-day months, so every
/// sidecar written in 2026 was stamped 17 days into the future (and the
/// drift grows by ~5 days a year). This is the proleptic-Gregorian
/// days-to-civil conversion (Howard Hinnant's algorithm), exact for every
/// date the `u64` can express; no dependency, no leap-second handling
/// (neither does `SystemTime`).
pub fn utc_timestamp_from_secs(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let z = days + 719_468; // shift the epoch from 1970-01-01 to 0000-03-01
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // year of era [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year, March-based [0, 365]
    let mp = (5 * doy + 2) / 153; // March = 0 ... February = 11
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

// ==================== Built-in Test Scenes ====================

/// Solid colors test scene - tests basic rectangle rendering
const SCENE_SOLID_COLORS: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Solid Colors Test</title>
    <style>
        body { margin: 0; padding: 20px; background: #ffffff; }
        .box { width: 100px; height: 100px; display: inline-block; margin: 10px; }
        .red { background: #ff0000; }
        .green { background: #00ff00; }
        .blue { background: #0000ff; }
        .yellow { background: #ffff00; }
        .cyan { background: #00ffff; }
        .magenta { background: #ff00ff; }
        .black { background: #000000; }
        .gray { background: #808080; }
    </style>
</head>
<body>
    <div class="box red"></div>
    <div class="box green"></div>
    <div class="box blue"></div>
    <div class="box yellow"></div>
    <br>
    <div class="box cyan"></div>
    <div class="box magenta"></div>
    <div class="box black"></div>
    <div class="box gray"></div>
</body>
</html>"#;

/// Basic text rendering test
const SCENE_TEXT_BASIC: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Text Basic Test</title>
    <style>
        body { 
            margin: 20px; 
            padding: 0; 
            background: #ffffff; 
            font-family: Arial, sans-serif;
        }
        .size-12 { font-size: 12px; }
        .size-16 { font-size: 16px; }
        .size-24 { font-size: 24px; }
        .size-32 { font-size: 32px; }
        .size-48 { font-size: 48px; }
        .bold { font-weight: bold; }
        .italic { font-style: italic; }
        .red { color: #ff0000; }
        .blue { color: #0000ff; }
        p { margin: 10px 0; }
    </style>
</head>
<body>
    <p class="size-12">Hello World - 12px</p>
    <p class="size-16">Hello World - 16px</p>
    <p class="size-24">Hello World - 24px</p>
    <p class="size-32 bold">Hello World - 32px Bold</p>
    <p class="size-48 red">Hello World - 48px Red</p>
    <p class="size-24 italic blue">Hello World - 24px Italic Blue</p>
    <p class="size-16">The quick brown fox jumps over the lazy dog.</p>
    <p class="size-16">ABCDEFGHIJKLMNOPQRSTUVWXYZ</p>
    <p class="size-16">abcdefghijklmnopqrstuvwxyz</p>
    <p class="size-16">0123456789 !@#$%^&*()</p>
</body>
</html>"#;

/// Glyph atlas test - renders specific glyphs for atlas verification
const SCENE_TEXT_GLYPH_ATLAS: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Glyph Atlas Test</title>
    <style>
        body { 
            margin: 20px; 
            padding: 0; 
            background: #ffffff; 
            font-family: Arial, sans-serif;
        }
        .glyph {
            font-size: 64px;
            display: inline-block;
            margin: 10px;
            padding: 10px;
            background: #f0f0f0;
            border: 1px solid #ccc;
        }
    </style>
</head>
<body>
    <div class="glyph">A</div>
    <div class="glyph">a</div>
    <div class="glyph">g</div>
    <div class="glyph">#</div>
    <div class="glyph">@</div>
    <div class="glyph">0</div>
    <div class="glyph">W</div>
    <div class="glyph">i</div>
</body>
</html>"#;

/// Alpha blending test
const SCENE_ALPHA_BLEND: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Alpha Blend Test</title>
    <style>
        body { margin: 0; padding: 20px; background: #ffffff; }
        .container { position: relative; width: 300px; height: 300px; }
        .box {
            position: absolute;
            width: 150px;
            height: 150px;
        }
        .box1 { 
            background: rgba(255, 0, 0, 0.7); 
            left: 0; 
            top: 0; 
        }
        .box2 { 
            background: rgba(0, 255, 0, 0.7); 
            left: 75px; 
            top: 75px; 
        }
        .box3 { 
            background: rgba(0, 0, 255, 0.7); 
            left: 150px; 
            top: 0; 
        }
        .box4 { 
            background: rgba(255, 255, 0, 0.5); 
            left: 75px; 
            top: 150px; 
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="box box1"></div>
        <div class="box box2"></div>
        <div class="box box3"></div>
        <div class="box box4"></div>
    </div>
</body>
</html>"#;

/// Border rendering test
const SCENE_BORDERS: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Borders Test</title>
    <style>
        body { margin: 20px; padding: 0; background: #ffffff; }
        .box {
            width: 100px;
            height: 100px;
            display: inline-block;
            margin: 10px;
            background: #f0f0f0;
        }
        .solid { border: 3px solid #000000; }
        .thick { border: 10px solid #ff0000; }
        .colored { 
            border-top: 5px solid red;
            border-right: 5px solid green;
            border-bottom: 5px solid blue;
            border-left: 5px solid yellow;
        }
        .rounded { border: 3px solid #000; border-radius: 20px; }
        .circle { border: 3px solid #000; border-radius: 50px; }
    </style>
</head>
<body>
    <div class="box solid"></div>
    <div class="box thick"></div>
    <div class="box colored"></div>
    <br>
    <div class="box rounded"></div>
    <div class="box circle"></div>
</body>
</html>"#;

/// Clip stack test
const SCENE_CLIP_STACK: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Clip Stack Test</title>
    <style>
        body { margin: 20px; padding: 0; background: #ffffff; }
        .outer {
            width: 200px;
            height: 200px;
            background: #ffcccc;
            overflow: hidden;
            border: 2px solid red;
        }
        .middle {
            width: 250px;
            height: 150px;
            background: #ccffcc;
            margin: 25px;
            overflow: hidden;
            border: 2px solid green;
        }
        .inner {
            width: 300px;
            height: 100px;
            background: #ccccff;
            margin: 25px;
            border: 2px solid blue;
        }
        .text {
            font-size: 14px;
            padding: 10px;
        }
    </style>
</head>
<body>
    <div class="outer">
        <div class="middle">
            <div class="inner">
                <div class="text">
                    This text should be clipped by nested overflow containers.
                    Only part of this content should be visible.
                </div>
            </div>
        </div>
    </div>
</body>
</html>"#;

/// Scrolling smoke test
const SCENE_SCROLLING_SMOKE: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Scrolling Smoke Test</title>
    <style>
        body { 
            margin: 0; 
            padding: 20px; 
            background: #ffffff; 
            font-family: Arial, sans-serif;
        }
        .scrollable {
            width: 300px;
            height: 200px;
            overflow: auto;
            border: 2px solid #333;
            background: #f9f9f9;
        }
        .content {
            padding: 10px;
        }
        .item {
            padding: 10px;
            margin: 5px;
            background: #e0e0e0;
            border-radius: 4px;
        }
        .item:nth-child(odd) { background: #d0d0ff; }
        .item:nth-child(even) { background: #d0ffd0; }
    </style>
</head>
<body>
    <h3>Scrollable Container</h3>
    <div class="scrollable">
        <div class="content">
            <div class="item">Item 1</div>
            <div class="item">Item 2</div>
            <div class="item">Item 3</div>
            <div class="item">Item 4</div>
            <div class="item">Item 5</div>
            <div class="item">Item 6</div>
            <div class="item">Item 7</div>
            <div class="item">Item 8</div>
            <div class="item">Item 9</div>
            <div class="item">Item 10</div>
            <div class="item">Item 11</div>
            <div class="item">Item 12</div>
        </div>
    </div>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_no_args() {
        // Would need to mock env::args - just test the struct defaults
        let config = ScreenshotConfig::default();
        assert!(config.gpu_capture);
        assert!(config.os_capture);
        assert_eq!(config.wait_frames, 3);
    }

    #[test]
    fn test_list_scenes() {
        let scenes = list_test_scenes();
        assert!(scenes.contains(&"text_basic"));
        assert!(scenes.contains(&"solid_colors"));
    }

    #[test]
    fn test_get_scene_html() {
        assert!(get_test_scene_html("text_basic").is_some());
        assert!(get_test_scene_html("invalid_scene").is_none());
    }
    #[test]
    fn sidecar_timestamp_is_a_real_civil_date() {
        // Values cross-checked against Python's datetime.fromtimestamp(..., UTC).
        assert_eq!(utc_timestamp_from_secs(0), "1970-01-01T00:00:00Z");
        // The capture that exposed the bug: the old helper stamped this
        // 2026-10-12T18:35:17Z, seventeen days late.
        assert_eq!(utc_timestamp_from_secs(1_790_361_317), "2026-09-25T18:35:17Z");
        // Leap day.
        assert_eq!(utc_timestamp_from_secs(1_709_208_000), "2024-02-29T12:00:00Z");
        // Leap day in a century year that IS a leap year.
        assert_eq!(utc_timestamp_from_secs(951_782_400), "2000-02-29T00:00:00Z");
        // Last second of 2099.
        assert_eq!(utc_timestamp_from_secs(4_102_444_799), "2099-12-31T23:59:59Z");
    }
}

