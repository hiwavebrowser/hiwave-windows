//! `--inspector-out`: render the HiWave Inspector page for a diagnostics
//! dump, headless, as RustKit draws it.
//!
//! The dump is the captured page's own (`Engine::all_element_diagnostics`
//! after the capture) or a file (`--inspector-dump`, as `--dump-diagnostics`
//! writes it). The Inspector is loaded into a fresh engine with script on,
//! its inline script is run, and the result is written to the output
//! directory as `inspector.html` (the page with its input, which opens in
//! any browser), `inspector.png` and `inspector.display-list.json`.

use rustkit_engine::{EngineBuilder, EngineConfig};
use rustkit_viewhost::Bounds;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// What `--inspector-out` wrote.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InspectorOutput {
    pub html_path: String,
    pub png_path: String,
    pub display_list_path: String,
    /// Records in the dump the Inspector shows.
    pub elements: usize,
    pub truncated: bool,
    /// Node id selected first (`--inspector-select-at`), if any.
    pub selected_node: Option<usize>,
    /// Wall-clock ms to parse and lay out the page (`load_html`), run its
    /// script (which builds the tree and lays out again), and paint.
    pub load_ms: u64,
    pub script_ms: u64,
    pub paint_ms: u64,
    /// Wall-clock ms of one ArrowDown after the capture: the next row's
    /// record parsed, the panes rebuilt, laid out and painted.
    pub arrow_down_ms: u64,
}

/// Build the Inspector page for `dump_json`, render it at `size` and write
/// the three files into `out_dir`.
pub fn render_inspector(
    dump_json: &str,
    diff_json: Option<&str>,
    select: Option<usize>,
    size: (u32, u32),
    out_dir: &Path,
) -> Result<InspectorOutput, String> {
    let page = rustkit_engine::inspector_page_html(dump_json, diff_json, select).map_err(|e| e.to_string())?;
    let dump: serde_json::Value = serde_json::from_str(dump_json).map_err(|e| e.to_string())?;
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let path = |name: &str| out_dir.join(name).to_string_lossy().into_owned();
    let (html_path, png_path, list_path, ppm_path) = (
        path("inspector.html"),
        path("inspector.png"),
        path("inspector.display-list.json"),
        path("inspector.ppm"),
    );
    fs::write(&html_path, &page).map_err(|e| format!("{html_path}: {e}"))?;

    let started = std::time::Instant::now();
    let mut engine = EngineBuilder::new()
        .with_config(EngineConfig::for_parity_testing())
        .user_agent("ParityCapture/1.0")
        .javascript_enabled(true)
        .build()
        .map_err(|e| format!("engine: {e:?}"))?;
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: size.0, height: size.1 })
        .map_err(|e| format!("view: {e:?}"))?;
    let ms = |t: std::time::Instant| t.elapsed().as_millis() as u64;
    // `load_html` runs the page's script; its time is part of the load.
    engine.load_html(view, &page).map_err(|e| format!("load: {e:?}"))?;
    let load_ms = ms(started);
    let log = engine.script_log(view).unwrap_or_default();
    if let Some(failed) = log.iter().find(|r| r.outcome != rustkit_engine::ScriptOutcome::Ran) {
        return Err(format!("inspector script: {} {:?}", failed.source, failed.outcome));
    }
    let script_ms = log.iter().map(|r| r.elapsed_ms).sum();
    let started = std::time::Instant::now();
    engine.render_view(view).map_err(|e| format!("render: {e:?}"))?;
    let paint_ms = ms(started);

    engine.capture_frame(view, &ppm_path).map_err(|e| format!("frame: {e:?}"))?;
    let png = ppm_to_png(&ppm_path, &png_path);
    let _ = fs::remove_file(&ppm_path);
    png?;
    engine
        .export_display_list_json(view, &list_path)
        .map_err(|e| format!("display list: {e:?}"))?;
    let started = std::time::Instant::now();
    engine.handle_text_key(view, 0x28, "ArrowDown", false, false, false);
    engine.render_view(view).map_err(|e| format!("render: {e:?}"))?;
    let arrow_down_ms = ms(started);
    let _ = engine.destroy_view(view);

    Ok(InspectorOutput {
        html_path,
        png_path,
        display_list_path: list_path,
        elements: dump["elements"].as_array().map_or(0, |e| e.len()),
        truncated: dump["truncated"].as_bool().unwrap_or(false),
        selected_node: select,
        load_ms,
        script_ms,
        paint_ms,
        arrow_down_ms,
    })
}

/// A binary PPM (`P6`, maxval 255) as an RGB PNG.
fn ppm_to_png(ppm_path: &str, png_path: &str) -> Result<(), String> {
    let bytes = fs::read(ppm_path).map_err(|e| format!("{ppm_path}: {e}"))?;
    let (width, height, pixels) = parse_ppm(&bytes).ok_or_else(|| format!("{ppm_path}: not a P6 PPM"))?;
    let file = fs::File::create(png_path).map_err(|e| format!("{png_path}: {e}"))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(pixels).map_err(|e| e.to_string())
}

fn parse_ppm(bytes: &[u8]) -> Option<(u32, u32, &[u8])> {
    // Header: "P6", width, height, maxval, separated by whitespace (and
    // `#` comments), then one whitespace byte before the pixels.
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'#') {
            if bytes[i] == b'#' {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            return None;
        }
        fields.push(std::str::from_utf8(&bytes[start..i]).ok()?);
    }
    if fields[0] != "P6" || fields[3] != "255" {
        return None;
    }
    let (w, h): (u32, u32) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
    let pixels = bytes.get(i + 1..i + 1 + (w as usize) * (h as usize) * 3)?;
    Some((w, h, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_p6_ppm_with_a_comment_parses() {
        let mut ppm = b"P6\n# made by a test\n2 1\n255\n".to_vec();
        ppm.extend_from_slice(&[255, 0, 0, 0, 0, 255]);
        let (w, h, px) = parse_ppm(&ppm).unwrap();
        assert_eq!((w, h, px), (2, 1, &[255u8, 0, 0, 0, 0, 255][..]));
        assert!(parse_ppm(b"P3\n1 1\n255\n1 2 3").is_none());
        assert!(parse_ppm(b"P6\n4 4\n255\n\x00").is_none(), "short pixel data");
    }
}
