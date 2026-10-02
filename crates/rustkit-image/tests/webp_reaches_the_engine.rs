//! The codec fix has to reach the ENGINE's decode path, not just its own
//! crate. `ImageManager::decode_bytes` routes through
//! `rustkit_codecs::decode_any`, so a WebP that decodes in the codec crate
//! but not here would mean the wiring, not the decoder, was the gap — the
//! orphan-shaped failure this codebase keeps producing.

use rustkit_image::ImageManager;

const TWO_HALVES: &[u8] = include_bytes!("fixtures/two_halves.webp");

#[test]
fn image_manager_decodes_webp_into_a_usable_image() {
    let mgr = ImageManager::new();
    let url = url::Url::parse("https://example.com/photo.webp").unwrap();

    let img = mgr
        .decode_bytes_for_test(&url, TWO_HALVES)
        .expect("engine image path must decode WebP");

    assert_eq!((img.natural_width, img.natural_height), (4, 2));
    // Content check, not just dimensions: a correctly-sized black field
    // would satisfy a size-only assertion while every photo rendered blank.
    match &img.data {
        rustkit_image::ImageData::Static(rgba) => {
            assert_eq!(&rgba.data()[0..4], &[255, 0, 0, 255], "red survives to the engine");
        }
        rustkit_image::ImageData::Animated(_) => panic!("still WebP must not be animated"),
    }
}
