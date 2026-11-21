//! MEDIA-AVF §3.3, frames step 1a: `Renderer::update_image` writes a video's new frame
//! into the texture already under its key when the size is the same, and makes a new
//! texture only when the size differs. The test skips, loudly, where no adapter exists.

use super::*;

fn renderer() -> Option<Renderer> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
            .ok()?;
    Renderer::new(
        Arc::new(device),
        Arc::new(queue),
        wgpu::TextureFormat::Bgra8Unorm,
    )
    .ok()
}

fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    (0..w * h)
        .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
        .collect()
}

#[test]
fn a_frame_of_the_same_size_is_written_into_the_same_texture() {
    let Some(mut r) = renderer() else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    let key = "rustkit-video:7@1";
    r.update_image(key, 64, 48, &solid(64, 48, [255, 0, 0]))
        .unwrap();
    let first = r.texture_cache().get(key).unwrap().texture.clone();
    r.update_image(key, 64, 48, &solid(64, 48, [0, 255, 0]))
        .unwrap();
    assert!(
        r.texture_cache().get(key).unwrap().texture == first,
        "written in place"
    );
    // A resolution switch is a new texture, of the new size.
    r.update_image(key, 32, 24, &solid(32, 24, [0, 0, 255]))
        .unwrap();
    let cached = r.texture_cache().get(key).unwrap();
    assert!(cached.texture != first);
    assert_eq!((cached.width, cached.height), (32, 24));
    assert_eq!(cached.texture.size().width, 32);
    // A frame whose bytes do not match its size is refused, and the texture kept.
    assert!(r
        .update_image(key, 64, 48, &solid(32, 24, [0, 0, 0]))
        .is_err());
    assert_eq!(r.texture_cache().get(key).unwrap().width, 32);
}
