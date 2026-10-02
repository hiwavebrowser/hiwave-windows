//! SHAPED-RUN CONTRACT, slice S0 (docs/SHAPED_RUN_CONTRACT_2026-09-30.md §7).
//!
//! The old path is the oracle: `shape_line_advances` and the `advances`
//! field are what develop ships, and every check here compares the frozen
//! run against them.

use super::*;
use crate::text::{PositionedGlyph, TextDirection};

fn style_in(family: &str, size: f32, weight: u16) -> ComputedStyle {
    let mut style = ComputedStyle::new();
    style.font_family = family.to_string();
    style.font_size = Length::Px(size);
    style.font_weight = rustkit_css::FontWeight(weight);
    style
}

/// The strings `kerning_deltas` was written for (pair kerning, one glyph
/// per UTF-16 unit), in the faces the campaign pages use.
fn kerning_corpus() -> Vec<(&'static str, ComputedStyle, f32)> {
    vec![
        (
            "CSS Specificity Test",
            style_in("system-ui", 32.0, 700),
            32.0,
        ),
        (
            "AVATAR To Wave, Yo. Type",
            style_in("Helvetica", 16.0, 400),
            16.0,
        ),
        (
            "The Art of Typography",
            style_in("Georgia, serif", 40.0, 700),
            40.0,
        ),
        (
            "The quick brown fox jumps over 42 lazy dogs.",
            style_in("Georgia, 'Times New Roman', serif", 17.6, 400),
            17.6,
        ),
    ]
}

/// root → block → one text run, laid out by hand; the display list of it.
fn one_line_list(text: &str, style: &ComputedStyle, block_style: ComputedStyle) -> DisplayList {
    let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    root.dimensions.content = Rect::new(0.0, 0.0, 800.0, 600.0);
    let mut block = LayoutBox::new(BoxType::Block, block_style);
    block.dimensions.content = Rect::new(10.0, 10.0, 120.0, 20.0);
    let mut run = LayoutBox::new(BoxType::Text(text.to_string()), style.clone());
    run.dimensions.content = Rect::new(10.0, 10.0, 120.0, 20.0);
    block.children.push(run);
    root.children.push(block);
    DisplayList::build(&root)
}

type TextParts = (
    String,
    f32,
    Option<Vec<f32>>,
    Option<std::sync::Arc<GlyphRun>>,
);

fn text_parts(list: &DisplayList) -> Vec<TextParts> {
    list.commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::Text {
                text,
                x,
                advances,
                run,
                ..
            } => Some((text.clone(), *x, advances.clone(), run.clone())),
            _ => None,
        })
        .collect()
}

/// §7 "Latin projection": the run's per-cluster advances ARE today's
/// vector, to the bit, and the pen walks the run exactly as the character
/// path walks that vector.
#[test]
#[cfg(target_os = "macos")]
fn a_latin_run_projects_onto_the_advances_layout_already_ships() {
    for (text, style, size) in kerning_corpus() {
        let old = shape_line_advances(text, &style, size).expect("the old path shapes Latin");
        let run = shape_line_run(text, &style, size, 0.0).expect("a Latin line has a run");

        assert_eq!(run.glyphs.len(), text.chars().count(), "{text}");
        assert_eq!(
            run.char_advances(text).as_deref(),
            Some(old.as_slice()),
            "{text}"
        );
        assert!(
            (run.width() - old.iter().sum::<f32>()).abs() < 1e-3,
            "{text}"
        );

        // Where the character path puts each glyph: x, then += advance.
        let mut cursor = 10.0f32;
        let old_pens: Vec<f32> = old
            .iter()
            .map(|a| {
                let here = cursor;
                cursor += a;
                here
            })
            .collect();
        assert_eq!(run.pen_positions(10.0), old_pens, "{text}");

        // One cluster per UTF-16 unit, in order, covering the text.
        let units: Vec<u32> = run.glyphs.iter().map(|g| g.cluster.start).collect();
        assert_eq!(
            units,
            (0..text.encode_utf16().count() as u32).collect::<Vec<_>>()
        );
        assert_eq!(run.direction, TextDirection::Ltr);
        assert_eq!(&run.script, b"Latn");
        assert!(run.variations.is_empty());
        assert!(run.ascent > 0.0 && run.descent > 0.0);
    }
}

/// Spacing and justification are in the run before it is frozen.
#[test]
#[cfg(target_os = "macos")]
fn spacing_and_justification_are_frozen_into_the_run() {
    let mut style = style_in("Helvetica", 16.0, 400);
    style.letter_spacing = Length::Px(2.0);
    style.word_spacing = Length::Px(3.0);
    let text = "two words here";
    let spaced = shape_line_advances(text, &style, 16.0).expect("shape");
    let run = shape_line_run(text, &style, 16.0, 0.0).expect("run");
    assert_eq!(run.char_advances(text).as_deref(), Some(spaced.as_slice()));

    // What the emitter does to the vector for a justified line.
    let justified: Vec<f32> = spaced
        .iter()
        .zip(text.chars())
        .map(|(a, c)| {
            if TextLine::is_word_separator(c) {
                a + 4.5
            } else {
                *a
            }
        })
        .collect();
    let run = shape_line_run(text, &style, 16.0, 4.5).expect("run");
    assert_eq!(
        run.char_advances(text).as_deref(),
        Some(justified.as_slice())
    );
}

/// A shaped line whose glyphs are not one per character: `office` with an
/// `ffi` ligature, as a shaper with ligatures on reports it. The macOS
/// shaper keeps ligatures off (`kerning_deltas`), so it never returns this
/// today; the run is built by hand from a real shape of the same string.
#[cfg(target_os = "macos")]
fn ligated_office() -> ShapedRun {
    let style = style_in("Helvetica", 16.0, 400);
    let mut shaped = shape_line("office", &style, 16.0).expect("shape");
    assert_eq!(shaped.glyphs.len(), 6);
    let merged: f32 = shaped.glyphs[1..4].iter().map(|g| g.advance).sum();
    shaped.glyphs[1].advance = merged;
    shaped.glyphs.drain(2..4);
    shaped
}

/// §7 "Non-1:1 cluster": the old function rejects the line (its `None`),
/// the frozen run gives the ligature one glyph over three code units, and
/// paint has a pen position for every glyph of the run.
#[test]
#[cfg(target_os = "macos")]
fn a_cluster_that_is_not_one_character_keeps_its_range() {
    let shaped = ligated_office();
    assert!(char_advances_of(&shaped).is_none(), "the old path's None");

    let run = GlyphRun::freeze(&shaped, 0.0).expect("the run exists");
    let ranges: Vec<_> = run.glyphs.iter().map(|g| g.cluster.clone()).collect();
    assert_eq!(ranges, vec![0..1, 1..4, 4..5, 5..6]);
    assert!(
        run.char_advances("office").is_none(),
        "no per-character vector"
    );
    assert_eq!(run.cluster_advances().len(), 4);

    let pens = run.pen_positions(0.0);
    assert_eq!(
        pens.len(),
        4,
        "paint places the run's glyphs, not characters"
    );
    assert!((pens[2] - (run.glyphs[0].advance + run.glyphs[1].advance)).abs() < 1e-4);
    assert!((run.width() - shaped.metrics.width).abs() < 1e-3);
}

/// Two glyphs of one cluster (a base and its mark) share one range, and a
/// character outside the BMP is one cluster two code units long.
#[test]
fn glyphs_of_one_cluster_share_a_range_and_ranges_count_utf16_units() {
    let glyph = |glyph_id, character, cluster, advance| PositionedGlyph {
        glyph_id,
        x: 0.0,
        y: 0.0,
        advance,
        character,
        cluster,
    };
    let shaped = ShapedRun {
        text: "e\u{301}\u{1D4B3}b".to_string(),
        glyphs: vec![
            glyph(5, 'e', 0, 8.0),
            glyph(6, '\u{301}', 0, 0.0),
            glyph(7, '\u{1D4B3}', 2, 11.0),
            glyph(8, 'b', 3, 9.0),
        ],
        font_family: "Test".to_string(),
        font_weight: rustkit_css::FontWeight(400),
        font_style: rustkit_css::FontStyle::Normal,
        font_stretch: rustkit_css::FontStretch::Normal,
        font_size: 16.0,
        metrics: TextMetrics::with_font_size(16.0),
        direction: TextDirection::Ltr,
        face: Some(FaceIdentity {
            id: 1,
            postscript_name: "Test-Regular".to_string(),
            face_index: 0,
        }),
    };
    let run = GlyphRun::freeze(&shaped, 0.0).expect("run");
    let ranges: Vec<_> = run.glyphs.iter().map(|g| g.cluster.clone()).collect();
    assert_eq!(ranges, vec![0..2, 0..2, 2..4, 4..5]);
    assert_eq!(
        run.cluster_advances(),
        vec![(0..2, 8.0), (2..4, 11.0), (4..5, 9.0)]
    );
    assert_eq!(run.pen_positions(1.0), vec![1.0, 9.0, 9.0, 20.0]);

    // A run without a named face, a right-to-left run, and a run with a
    // glyph the face lacks are outside the slice.
    let mut unnamed = shaped.clone();
    unnamed.face = None;
    assert!(GlyphRun::freeze(&unnamed, 0.0).is_none());
    let mut rtl = shaped.clone();
    rtl.direction = TextDirection::Rtl;
    assert!(GlyphRun::freeze(&rtl, 0.0).is_none());
    let mut missing = shaped;
    missing.glyphs[3].glyph_id = 0;
    assert!(GlyphRun::freeze(&missing, 0.0).is_none());
}

/// §7 "Face identity, macOS": the run records the face the family list
/// resolved to, per weight, and skips a family that is not installed.
#[test]
#[cfg(target_os = "macos")]
fn the_run_names_the_face_the_family_list_resolved_to() {
    let list = "Georgia, 'Times New Roman', serif";
    let regular = shape_line_run("Wave", &style_in(list, 24.0, 400), 24.0, 0.0).expect("run");
    assert_eq!(regular.face.postscript_name, "Georgia");
    let bold = shape_line_run("Wave", &style_in(list, 24.0, 700), 24.0, 0.0).expect("run");
    assert_eq!(bold.face.postscript_name, "Georgia-Bold");
    assert_ne!(regular.face.id, bold.face.id, "two faces, two ids");

    let walked = style_in("No Such Family 9f2c, Georgia", 24.0, 400);
    let walked = shape_line_run("Wave", &walked, 24.0, 0.0).expect("run");
    assert_eq!(
        walked.face, regular.face,
        "the same face under another list"
    );

    let other = shape_line_run("Wave", &style_in("Helvetica", 24.0, 400), 24.0, 0.0).expect("run");
    assert_ne!(other.face.id, regular.face.id);

    // The font layout shaped with is what the rasterizer is handed.
    let font = rustkit_text::macos::face_font(regular.face.id, 24.0).expect("the face is held");
    assert_eq!(font.postscript_name(), "Georgia");
    assert!(rustkit_text::macos::face_font(regular.face.id ^ 0x5a5a, 24.0).is_none());
}

/// The emitter: the Text command carries the run, and `advances` is the
/// run's projection.
#[test]
#[cfg(target_os = "macos")]
fn the_text_command_carries_the_run_and_its_projection() {
    let style = style_in("Georgia, 'Times New Roman', serif", 16.0, 400);
    let list = one_line_list("Wave To", &style, ComputedStyle::new());
    let texts = text_parts(&list);
    assert_eq!(texts.len(), 1, "{texts:?}");
    let (text, _, advances, run) = &texts[0];
    let run = run.as_ref().expect("a Latin line carries its run");
    assert_eq!(run.face.postscript_name, "Georgia");
    assert_eq!(run.font_size, 16.0);
    assert_eq!(run.char_advances(text), *advances);
    assert_eq!(
        *advances,
        shape_line_advances(text, &style, 16.0),
        "the vector is what develop ships"
    );
}

/// A character the face cannot draw keeps the line on today's path: no
/// run, and the advance vector the character path paints from.
#[test]
#[cfg(target_os = "macos")]
fn a_line_with_a_fallback_character_keeps_the_character_path() {
    let style = style_in("Helvetica", 16.0, 400);
    for text in ["\u{2615} coffee", "caf\u{e9} \u{4e2d}\u{6587}"] {
        let list = one_line_list(text, &style, ComputedStyle::new());
        let texts = text_parts(&list);
        let (_, _, advances, run) = &texts[0];
        assert!(run.is_none(), "{text}: outside S0");
        assert_eq!(*advances, shape_line_advances(text, &style, 16.0), "{text}");
    }
}

/// `text-overflow: ellipsis` cuts the run where it cuts the characters.
#[test]
#[cfg(target_os = "macos")]
fn an_ellipsis_cut_cuts_the_run_with_the_characters() {
    let mut style = style_in("Helvetica", 16.0, 400);
    style.white_space = rustkit_css::WhiteSpace::Nowrap;
    let mut block_style = style.clone();
    block_style.overflow_x = rustkit_css::Overflow::Hidden;
    block_style.overflow_y = rustkit_css::Overflow::Hidden;
    block_style.text_overflow = rustkit_css::TextOverflow::Ellipsis;
    let list = one_line_list(
        "The quick brown fox jumps over the lazy dog",
        &style,
        block_style,
    );
    let texts = text_parts(&list);
    assert_eq!(texts.len(), 1, "{texts:?}");
    let (text, _, advances, run) = &texts[0];
    assert!(
        text.ends_with('\u{2026}') && text.chars().count() > 3,
        "{text:?}"
    );
    let run = run.as_ref().expect("the cut line carries its run");
    assert_eq!(run.glyphs.len(), text.chars().count());
    assert_eq!(run.char_advances(text), *advances);
    assert_eq!(
        run.glyphs.last().map(|g| g.cluster.end),
        Some(text.encode_utf16().count() as u32)
    );
}
