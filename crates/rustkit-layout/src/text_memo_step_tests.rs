//! TEXT-CACHE-STEP: a page with more distinct strings than a text memo
//! holds must not re-shape all of them on every pass.
//!
//! Both per-thread text memos (measurement in `measure_text_with_spacing`,
//! shaping in `TextShaper::shape`) were wiped completely when full. A pass
//! over `cap + n` distinct strings filled the memo, wiped it, refilled it,
//! and the next pass found almost nothing: every string was shaped again.
//! Forced layout per element stepped from ~7 to ~11 us between 16,300 and
//! 16,500 text rows (Windows measurement seat; Mac numbers in the PR).
//!
//! The counts here are shapes, not time. The web-font generation is
//! process-wide and other tests install web fonts on their threads, which
//! drops every memo; a round is only judged when the generation held still.

use crate::text::{
    shape_memo_misses, shaped_memo_bytes, FontFamilyChain, TextShaper, SHAPED_TEXT_MEMO_BYTES,
};
use crate::{measure_text_with_spacing, TEXT_MEASURE_MEMO_CAP, TEXT_SHAPES};
use rustkit_css::{FontStretch, FontStyle, FontWeight};

/// Distinct strings beyond the cap.
const OVER: usize = 1000;

/// The entry cap the shaped-text memo had before SHAPED-CAP (16,384); it is
/// bounded by bytes now (`SHAPED_TEXT_MEMO_BYTES`).
const OLD_SHAPED_CAP: usize = 16_384;

fn row_text(i: usize) -> String {
    format!("row {}", 100_000 + i)
}

/// Runs `round` (one pass over the strings) until a pass happens with the
/// web-font generation unchanged; returns that pass's shape count.
fn steady_pass(mut round: impl FnMut() -> usize) -> usize {
    for _ in 0..10 {
        let generation = rustkit_text::webfonts::generation();
        let n = round();
        if rustkit_text::webfonts::generation() == generation {
            return n;
        }
    }
    panic!("the web-font generation never held still");
}

#[test]
fn a_second_measure_pass_over_more_strings_than_the_cap_reshapes_only_the_overflow() {
    let n = TEXT_MEASURE_MEMO_CAP + OVER;
    let pass = || {
        let before = TEXT_SHAPES.with(std::cell::Cell::get);
        for i in 0..n {
            measure_text_with_spacing(
                &row_text(i),
                "sans-serif",
                16.0,
                FontWeight(400),
                FontStyle::Normal,
                0.0,
                0.0,
            );
        }
        TEXT_SHAPES.with(std::cell::Cell::get) - before
    };
    let second = steady_pass(|| {
        pass();
        pass()
    });
    // Clear-when-full re-measured all of them (about `n`); the memo keeps
    // the `cap` it holds and measures only what does not fit.
    assert!(
        second <= OVER + OVER / 10,
        "second pass over {n} distinct strings measured {second} of them again (cap {TEXT_MEASURE_MEMO_CAP})"
    );
}

#[test]
fn a_second_shape_pass_over_more_strings_than_the_cap_reshapes_only_the_overflow() {
    let n = OLD_SHAPED_CAP + OVER;
    let shaper = TextShaper::new();
    let chain = FontFamilyChain::from_css_value("sans-serif");
    let pass = || {
        let before = shape_memo_misses();
        for i in 0..n {
            shaper
                .shape(
                    &row_text(i),
                    &chain,
                    FontWeight(400),
                    FontStyle::Normal,
                    FontStretch::Normal,
                    16.0,
                )
                .expect("shape");
        }
        shape_memo_misses() - before
    };
    let second = steady_pass(|| {
        pass();
        pass()
    });
    assert!(
        second <= OVER + OVER / 10,
        "second pass over {n} distinct strings shaped {second} of them again (the old cap was {OLD_SHAPED_CAP})"
    );
}

#[test]
fn a_relayout_over_rows_shaped_under_two_keys_reshapes_none_the_second_time() {
    // SHAPED-CAP: a relayout shapes each row at least twice under different
    // keys (layout, then the display list), so `rows` text rows need
    // `2 x rows` entries. Past 2 x 16,384 entries the clock's hand lapped
    // within a pass and evicted what it was keeping (fl-19977-u5 display
    // list 89.6 ms vs r5 35 ms on the Mac). Rows here are short strings
    // like the fixtures', well inside the byte budget, so the second pass
    // finds every one.
    let rows = OLD_SHAPED_CAP + OVER;
    let shaper = TextShaper::new();
    let chain = FontFamilyChain::from_css_value("sans-serif");
    let pass = || {
        let before = shape_memo_misses();
        for i in 0..rows {
            for size in [16.0, 13.0] {
                shaper
                    .shape(
                        &row_text(i),
                        &chain,
                        FontWeight(400),
                        FontStyle::Normal,
                        FontStretch::Normal,
                        size,
                    )
                    .expect("shape");
            }
        }
        shape_memo_misses() - before
    };
    let second = steady_pass(|| {
        pass();
        pass()
    });
    assert_eq!(
        second, 0,
        "second pass over {rows} rows x 2 keys shaped {second} again"
    );
}

#[test]
fn the_shaped_memo_stays_within_its_byte_budget_through_churn_with_long_strings() {
    // Long strings weigh far more than short ones: 600 distinct 3,000-char
    // strings (about 75 KB of glyphs each, about 45 MB in all) through a
    // 32 MB memo, twice. The memo must stay within its bytes and still hold
    // most of what fits.
    let shaper = TextShaper::new();
    let chain = FontFamilyChain::from_css_value("sans-serif");
    let long = |i: usize| format!("{i:06} ").repeat(375);
    let mut peak = 0;
    let generation = rustkit_text::webfonts::generation();
    for _ in 0..2 {
        for i in 0..600 {
            shaper
                .shape(
                    &long(i),
                    &chain,
                    FontWeight(400),
                    FontStyle::Normal,
                    FontStretch::Normal,
                    16.0,
                )
                .expect("shape");
            let bytes = shaped_memo_bytes();
            assert!(
                bytes <= SHAPED_TEXT_MEMO_BYTES,
                "shaped memo holds {bytes} bytes, budget {SHAPED_TEXT_MEMO_BYTES}"
            );
            peak = peak.max(bytes);
        }
    }
    if rustkit_text::webfonts::generation() == generation {
        assert!(
            peak > SHAPED_TEXT_MEMO_BYTES / 2,
            "the memo never filled: peak {peak} bytes"
        );
    }
}

/// Every box's geometry, in tree order.
fn geometry(b: &crate::LayoutBox, out: &mut Vec<String>) {
    out.push(format!("{:?}", b.dimensions));
    for c in &b.children {
        geometry(c, out);
    }
}

/// A block of `n` rows, each a block holding one distinct text.
fn rows(n: usize) -> crate::LayoutBox {
    use crate::{BoxType, LayoutBox};
    let style = rustkit_css::ComputedStyle::new();
    let mut root = LayoutBox::new(BoxType::Block, style.clone());
    root.children = (0..n)
        .map(|i| {
            let mut row = LayoutBox::new(BoxType::Block, style.clone());
            row.children
                .push(LayoutBox::new(BoxType::Text(row_text(i)), style.clone()));
            row
        })
        .collect();
    root
}

fn laid_out(n: usize) -> Vec<String> {
    let mut root = rows(n);
    let viewport = crate::Dimensions {
        content: crate::Rect::new(0.0, 0.0, 1280.0, 800.0),
        ..Default::default()
    };
    root.layout(&viewport);
    let mut out = Vec::new();
    geometry(&root, &mut out);
    out
}

#[test]
fn geometry_is_the_same_with_the_text_memos_bypassed_cold_and_warm() {
    // More distinct strings than either memo holds, so the warm pass mixes
    // hits with strings the memo had to turn away.
    let n = TEXT_MEASURE_MEMO_CAP.max(OLD_SHAPED_CAP) + OVER;
    let uncached = crate::text_memo::without_text_memos(|| laid_out(n));
    let before = TEXT_SHAPES.with(std::cell::Cell::get);
    let cold = laid_out(n);
    assert!(
        TEXT_SHAPES.with(std::cell::Cell::get) - before >= OVER,
        "the layout never reached the measurement memo"
    );
    let warm = laid_out(n);
    assert_eq!(uncached.len(), cold.len());
    for (i, (u, c)) in uncached.iter().zip(&cold).enumerate() {
        assert_eq!(u, c, "box {i}: uncached vs first cached pass");
    }
    for (i, (u, w)) in uncached.iter().zip(&warm).enumerate() {
        assert_eq!(u, w, "box {i}: uncached vs second cached pass");
    }
}
