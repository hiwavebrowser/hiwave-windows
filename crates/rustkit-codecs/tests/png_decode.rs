use rustkit_codecs::decode_png;

fn encode_rgba(width: u32, height: u32) -> (Vec<u8>, Vec<u8>) {
    let pixels: Vec<_> = (0..width * height)
        .flat_map(|p| [p as u8, (p / width) as u8, 64, (p >> 3) as u8])
        .collect();
    (
        encode_png(width, height, png::ColorType::Rgba, &pixels),
        pixels,
    )
}

fn encode_png(width: u32, height: u32, color: png::ColorType, pixels: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, width, height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(pixels)
        .unwrap();
    encoded
}

const EXPANDED_FORMATS: [png::ColorType; 3] = [
    png::ColorType::Rgb,
    png::ColorType::Grayscale,
    png::ColorType::GrayscaleAlpha,
];

#[test]
fn png_channel_expansion_preserves_each_pixel() {
    for color in EXPANDED_FORMATS {
        let pixels: Vec<_> = (0..37 * 23 * color.samples()).map(|i| i as u8).collect();
        let bytes = encode_png(37, 23, color, &pixels);
        let image = decode_png(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (37, 23));
        for (input, output) in pixels
            .chunks_exact(color.samples())
            .zip(image.data().chunks_exact(4))
        {
            let expected = match input {
                &[r, g, b] => [r, g, b, 255],
                &[g] => [g, g, g, 255],
                &[g, a] => [g, g, g, a],
                _ => unreachable!(),
            };
            assert_eq!(output, expected);
        }
    }
}

#[test]
#[ignore = "release-mode PNG decode timing"]
fn expanded_png_decode_timing() {
    for color in EXPANDED_FORMATS {
        let pixels: Vec<_> = (0..1920 * 1080 * color.samples())
            .map(|i| i as u8)
            .collect();
        let bytes = encode_png(1920, 1080, color, &pixels);
        let mut samples = Vec::new();
        for _ in 0..21 {
            let start = std::time::Instant::now();
            let image = decode_png(std::hint::black_box(&bytes)).unwrap();
            samples.push(start.elapsed());
            std::hint::black_box(image);
        }
        samples.sort_unstable();
        eprintln!(
            "1080p {color:?} decode median {:?}",
            samples[samples.len() / 2]
        );
    }
}

#[test]
fn rgba_png_preserves_dimensions_and_pixel_bytes() {
    let (bytes, expected) = encode_rgba(37, 23);
    let image = decode_png(&bytes).unwrap();
    assert_eq!((image.width(), image.height()), (37, 23));
    assert_eq!(image.data(), expected);
}

#[test]
#[ignore = "release-mode PNG decode timing"]
fn rgba_png_decode_timing() {
    let (bytes, _) = encode_rgba(1920, 1080);
    let mut samples = Vec::new();
    for _ in 0..21 {
        let start = std::time::Instant::now();
        let image = decode_png(std::hint::black_box(&bytes)).unwrap();
        samples.push(start.elapsed());
        std::hint::black_box(image);
    }
    samples.sort_unstable();
    eprintln!("1080p RGBA decode median {:?}", samples[samples.len() / 2]);
}
