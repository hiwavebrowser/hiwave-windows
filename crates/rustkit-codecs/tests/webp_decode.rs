//! WebP decode — the format the live web actually serves photos in.
//!
//! Before this, `detect_format` recognised WebP and `decode_any` returned
//! `Unsupported`: detection without decoding, which reads in a log like a
//! missing feature rather than a missing decoder. Every eBay listing photo
//! failed this way (82 in one page of Pete's 2026-08-07 session).
//!
//! Fixtures are REAL WebP bytes (lossless, VP8L) generated at authoring
//! time, not hand-built headers — a fake fixture would prove the test
//! harness works and nothing about the decoder.

use rustkit_codecs::{decode_any, detect_format, Decoded, ImageFormat};

const TWO_HALVES: &[u8] = include_bytes!("fixtures/two_halves.webp");
const WITH_ALPHA: &[u8] = include_bytes!("fixtures/alpha.webp");

#[test]
fn detection_and_decoding_agree_on_webp() {
    // The precondition that made the old failure confusing: detection was
    // already correct. If this ever regresses the test below tests nothing.
    assert_eq!(detect_format(TWO_HALVES), Some(ImageFormat::WebP));

    let decoded = decode_any(TWO_HALVES).expect("WebP must decode, not report Unsupported");
    let img = match decoded {
        Decoded::Static(img) => img,
        Decoded::Animated(_) => panic!("a still WebP must not decode as animated"),
    };
    assert_eq!((img.width(), img.height()), (4, 2));
}

#[test]
fn pixels_survive_the_decode_in_rgba_order() {
    // Asserting CONTENT, not just "it returned Ok": a decoder that hands
    // back a correctly-sized field of zeros would pass a dimensions-only
    // test while every photo rendered black.
    let Decoded::Static(img) = decode_any(TWO_HALVES).expect("decode") else {
        panic!("static");
    };
    let px = img.data();
    assert_eq!(px.len(), 4 * 2 * 4, "RGBA8 stride");

    // Left half red, right half blue — and the channel ORDER is the thing
    // most likely to be silently wrong (BGRA vs RGBA), which a
    // single-colour fixture could not catch.
    assert_eq!(&px[0..4], &[255, 0, 0, 255], "top-left is opaque red");
    assert_eq!(&px[8..12], &[0, 0, 255, 255], "third pixel is opaque blue");
}

#[test]
fn alpha_channel_is_preserved_not_flattened() {
    // The RGB->RGBA fill path is only correct when it does NOT run for
    // images that carry their own alpha. Flattening here would make every
    // transparent logo opaque.
    let Decoded::Static(img) = decode_any(WITH_ALPHA).expect("decode") else {
        panic!("static");
    };
    let px = img.data();
    assert_eq!(px[3], 255, "first pixel opaque");
    assert_eq!(px[7], 0, "second pixel fully transparent — alpha survived");
}
