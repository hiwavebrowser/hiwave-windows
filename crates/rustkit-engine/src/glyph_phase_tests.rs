//! TX3: glyphs are rasterized at the subpixel phase Chrome draws them at.
//!
//! Skia (Chromium 148, its DEPS pin 03c3234e): a horizontal run's glyph
//! origin is quantized to 4 horizontal phases and a whole-pixel baseline,
//! `SkGlyphPositionRoundingSpec::HalfAxisSampleFreq` for `kX` being
//! `{kSubpixelRound = 1/8, 1/2}` with `kSubPixelPosLen = 2`: x + 1/8 is
//! floored to a quarter pixel (round to the nearest quarter), y + 1/2 to a
//! whole pixel. CfT 148 on macOS does this under the parity flags: the same
//! line at left 8 + {0.125, 0.25} draws identical ink, 8 + 0.875 draws the
//! 8 + 0 ink one column right.
//!
//! RustKit rasterized every glyph at phase 0 with its outline on a whole
//! bitmap column and placed the bitmap at the fractional bearing, so the
//! atlas was sampled between texels: a dilated edge (#1030) smeared into
//! the column before it (batch 56: css-text line-break-anywhere-001/-002,
//! overflow-wrap-anywhere-003 went FAIL; Chrome's own pairs are 0 px).
//!
//! Expected values are Chrome for Testing 148.0.7778.216, headless, parity
//! flags, on the same pages at 240x60 (13px Arial / 13px Courier).

use super::*;

const WIDTH: u32 = 240;
const HEIGHT: u32 = 60;

fn page_frame(html: &str) -> Vec<u8> {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: WIDTH, height: HEIGHT })
        .expect("view");
    engine.load_html(view, html).expect("load_html");
    engine.render_view(view).expect("render");
    let path = std::env::temp_dir().join(format!(
        "rustkit-glyph-phase-{}-{:?}.ppm",
        std::process::id(),
        std::thread::current().id()
    ));
    engine.capture_frame(view, path.to_str().unwrap()).expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm[ppm.len() - WIDTH as usize * HEIGHT as usize * 3..].to_vec()
}

fn differing(a: &[u8], b: &[u8]) -> Vec<(usize, usize)> {
    a.chunks_exact(3)
        .zip(b.chunks_exact(3))
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .map(|(i, _)| (i % WIDTH as usize, i / WIDTH as usize))
        .collect()
}

/// Ink per column (sum over rows of 255 - green) of black text on white.
fn ink_columns(px: &[u8]) -> Vec<u32> {
    let mut cols = vec![0u32; WIDTH as usize];
    for (i, p) in px.chunks_exact(3).enumerate() {
        cols[i % WIDTH as usize] += 255 - p[1] as u32;
    }
    cols
}

fn line_at(offset: &str) -> Vec<u32> {
    let left = 8.0 + offset.parse::<f32>().expect("offset");
    ink_columns(&page_frame(&format!(
        "<!doctype html><html><body style=\"margin:0;background:#fff\"><div style=\"position:absolute;left:{left}px;top:8px;font:13px Arial;color:#000;line-height:16px;white-space:nowrap\">Full automation</div></body></html>"
    )))
}

fn first_ink(cols: &[u32]) -> usize {
    cols.iter().position(|c| *c > 0).expect("some ink")
}

/// Chrome 148, "Full automation" 13px Arial at left 8 + offset: the first
/// ink column and the ink of the first four columns from there.
const CHROME: [(&str, usize, [u32; 4]); 4] = [
    ("0", 8, [19, 2402, 1228, 603]),
    ("0.25", 9, [1754, 1686, 603, 603]),
    ("0.5", 9, [1140, 2185, 603, 603]),
    ("0.75", 9, [556, 2402, 800, 603]),
];

/// The leading edge of the line at each quarter phase is where Chrome puts
/// it, with Chrome's coverage (each column within 15% + 60 of Chrome's sum).
#[test]
fn a_line_at_each_quarter_phase_has_chromes_leading_columns() {
    for (offset, first, cols) in CHROME {
        let got = line_at(offset);
        let at = first_ink(&got);
        assert_eq!(at, first, "offset {offset}: first ink column {at}, Chrome {first}; {:?}", &got[at..at + 4]);
        for (k, want) in cols.iter().enumerate() {
            let have = got[first + k];
            let tolerance = want * 15 / 100 + 60;
            assert!(
                have.abs_diff(*want) <= tolerance,
                "offset {offset}: column {} ink {have}, Chrome {want}; {:?}",
                first + k,
                &got[first..first + 4]
            );
        }
    }
}

/// Skia rounds each glyph's origin to the nearest quarter pixel: the first
/// glyph ("F", columns 8..16) at 0.125 draws as at 0.25, and at 0.875 as at
/// 1.0, i.e. the 0 glyph one column right. (Later glyphs round from their
/// own pens, so the rest of the line differs between 0.125 and 0.25, in
/// Chrome 148 too.)
#[test]
fn origins_round_to_the_nearest_quarter_pixel() {
    assert_eq!(&line_at("0.125")[8..16], &line_at("0.25")[8..16], "0.125 draws as 0.25");
    let zero = line_at("0");
    let next = line_at("0.875");
    assert_eq!(&next[9..17], &zero[8..16], "0.875 draws as 1.0");
}

/// Chrome 148's ink per column for the whole line at 0.25, columns 0..100.
const CHROME_LINE_0_25: [u32; 100] = [0,0,0,0,0,0,0,0,0,1754,1686,603,603,603,599,196,0,1535,764,296,302,1250,1024,0,1716,1226,0,2401,642,0,0,0,0,570,1218,881,872,1075,1623,234,297,1623,431,289,317,1629,593,144,1621,1469,387,421,1330,700,598,681,1271,675,335,1732,429,289,637,1612,436,293,613,1557,70,302,1248,925,856,932,1612,592,144,1621,1469,387,893,1722,0,721,1230,641,611,761,1342,380,335,1732,416,297,346,1534,538,0,0,0];

/// The whole line at 0.25, every glyph at its own rounded phase: each
/// column within 15% + 60 of Chrome's.
#[test]
fn the_whole_line_at_a_quarter_phase_matches_chromes_columns() {
    let got = line_at("0.25");
    let off: Vec<(usize, u32, u32)> = CHROME_LINE_0_25
        .iter()
        .enumerate()
        .filter(|(i, want)| got[*i].abs_diff(**want) > **want * 15 / 100 + 60)
        .map(|(i, want)| (i, got[i], *want))
        .collect();
    assert!(off.is_empty(), "columns (col, RustKit, Chrome) off: {off:?}");
}

/// Reduced from WPT css-text line-break-anywhere-001 (13px Courier, as
/// Chrome draws bare monospace): Chrome 148's test and reference are
/// identical (0 px).
#[test]
fn a_glyph_under_a_one_ch_cover_does_not_peek_out() {
    let st = "<style>body{margin:8px;background:#fff}#green{position:absolute;background:green;font:13px Courier;width:1ch;height:19em}#test{width:1ch;line-height:1;color:red;font:13px Courier;line-break:anywhere}</style>";
    let test = page_frame(&format!("<!doctype html><html>{st}<div id=green></div><div id=test>aa-a.a)a,aaaaaaa</div></html>"));
    let reference = page_frame(&format!("<!doctype html><html>{st}<div id=green></div></html>"));
    let d = differing(&test, &reference);
    assert!(d.is_empty(), "{} px differ (Chrome 148: 0): {:?}", d.len(), &d[..d.len().min(8)]);
}

/// Reduced from WPT css-text overflow-wrap-anywhere-003: "PASS" exactly
/// fills a 4ch `overflow: hidden` box vs unclipped. Chrome 148: 0 px.
#[test]
fn a_clip_at_the_text_edge_cuts_no_glyph_ink() {
    let test = page_frame("<!doctype html><html><body style=\"margin:0;background:#fff\"><div style=\"position:absolute;left:8px;top:8px;font:13px Courier;width:4ch;line-height:1;overflow:hidden;height:1em\">PASS</div></body></html>");
    let reference = page_frame("<!doctype html><html><body style=\"margin:0;background:#fff\"><div style=\"position:absolute;left:8px;top:8px;font:13px Courier;line-height:1\">PASS</div></body></html>");
    let d = differing(&test, &reference);
    assert!(d.is_empty(), "{} px differ (Chrome 148: 0): {:?}", d.len(), &d[..d.len().min(8)]);
}
