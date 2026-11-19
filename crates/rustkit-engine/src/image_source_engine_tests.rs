//! E54b: the engine wires `image_source::select_source` into layout: an `<img>`'s box is the
//! picked image's decoded size over its density (a 200 px image picked as 2x is 100 CSS px wide,
//! as Chrome's `naturalWidth` says), the `<picture>` source is used, and a page that offers only
//! `srcset` still shows an image. The images are `data:` PNGs of 100, 200 and 400 px width.

use super::*;

const P100: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAGQAAAAyCAIAAAAlV+npAAAAX0lEQVR42u3QMQEAAAQAMJwSiyiWCgJsEZY9G/yUAlmyZMmSJUuBLFmyZMmSpUCWLFmyZMlSIEuWLFmyZCmQJUuWLFmyFMiSJUuWLFkKZMmSJUuWLAWyZMmSJUuWgr8DgGkBrsCUGxcAAAAASUVORK5CYII=";
const P200: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAMgAAABkCAIAAABM5OhcAAAAy0lEQVR42u3SMREAMAgAsdIRxUhEFiYYGBIJfx9Z/WDblwBjYSyMBcbCWBgLjIWxMBYYC2NhLDAWxsJYYCyMhbHAWBgLY4GxMBbGAmNhLIwFxsJYGAuMhbEwFhgLY2EsMBbGwlhgLIyFscBYGAtjgbEwFsYCY2EsjAXGwlgYC4yFsTAWGAtjYSwwFsbCWGAsjIWxwFgYC2OBsTAWxgJjYSyMBcbCWBgLjIWxMBYYC2NhLDAWxsJYGAuMhbEwFhgLY2EsMBbGwlhgLG4bU88CElBOZOUAAAAASUVORK5CYII=";
const P400: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAZAAAADICAIAAABJdyC1AAACBElEQVR42u3UMQ0AQAjAwOdHFCMRWahgILmT0KGR1Q/ggi8BYFgAhgUYFoBhARgWYFgAhgVgWIBhARgWgGEBhgVgWACGBRgWgGEBGBZgWACGBWBYgGEBGBaAYQGGBWBYAIYFGBaAYQGGBWBYAIYFGBaAYQEYFmBYAIYFYFiAYQEYFoBhAYYFYFgAhgUYFoBhARgWYFgAhgVgWIBhARgWgGEBhgVgWACGBRgWgGEBhgVgWACGBRgWgGEBGBZgWACGBWBYgGEBGBaAYQGGBWBYAIYFGBaAYQEYFmBYAIYFYFiAYQEYFoBhAYYFYFgAhgUYFoBhAYYFYFgAhgUYFoBhARgWYFgAhgVgWIBhARgWgGEBhgVgWACGBRgWgGEBGBZgWACGBWBYgGEBGBaAYQGGBWBYAIYFGBaAYQGGBWBYAIYFGBaAYQEYFmBYAIYFYFiAYQEYFoBhAYYFYFgAhgUYFoBhARgWYFgAhgVgWIBhARgWgGEBhgVgWACGBRgWgGEBhgVgWACGBRgWgGEBGBZgWACGBWBYgGEBGBaAYQGGBWBYAIYFGBaAYQEYFmBYAIYFYFiAYQEYFoBhAYYFYFgAhgUYFoBhAYYFYFgAhgUYFoBhARgWYFgAhgVgWIBhARgWgGEBhgVgWACGBRgWgGEBGBZgWACGBWBYgGEBGBaAYQGGBbBnAKIMAtqV+NfbAAAAAElFTkSuQmCC";

fn width_of(page: &str, dpr: f32, viewport_w: u32) -> f64 {
    let config = EngineConfig { device_pixel_ratio: dpr, ..EngineConfig::default() };
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: viewport_w, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load_html");
    let out = engine
        .execute_script(view, "String(document.getElementById('t').getBoundingClientRect().width)")
        .unwrap();
    out.strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&out)
        .parse()
        .unwrap_or(f64::NAN)
}

fn page(body: String) -> String {
    format!("<html><body style='margin:0'>{body}</body></html>")
}

#[test]
fn a_2x_pick_of_a_200px_image_is_100_css_px_wide() {
    let body = format!("<img id=t srcset=\"{P100} 1x, {P200} 2x\">");
    let w = width_of(&page(body.clone()), 2.0, 800);
    assert!((w - 100.0).abs() < 0.5, "at DPR 2 the 2x image is 100 CSS px, got {w}");
    assert!((width_of(&page(body), 1.0, 800) - 100.0).abs() < 0.5, "at DPR 1 the 1x image is 100 CSS px");
}

#[test]
fn a_page_with_only_a_srcset_still_shows_an_image() {
    // The original hole: no src, one 2x candidate. It is picked at DPR 1 (the largest covers
    // nothing smaller) and its box is the decoded 200 px over density 2.
    let body = format!("<img id=t srcset=\"{P200} 2x\">");
    assert!((width_of(&page(body), 1.0, 800) - 100.0).abs() < 0.5);
}

#[test]
fn a_picture_source_is_used_when_its_media_matches() {
    let body = format!(
        "<picture><source media=\"(min-width: 600px)\" srcset=\"{P400}\"><img id=t src=\"{P100}\"></picture>"
    );
    assert!((width_of(&page(body.clone()), 1.0, 800) - 400.0).abs() < 0.5, "a wide viewport takes the source");
    assert!((width_of(&page(body), 1.0, 400) - 100.0).abs() < 0.5, "a narrow one takes the img's src");
}

#[test]
fn a_pending_swap_keeps_the_old_images_own_density() {
    // Prometheus nit 1 on #1039: during a pending swap (LAT9) the box paints the previously shown
    // image, so its natural size must divide by THAT image's density. A lazy loader that writes a
    // `w` srcset over a 1x placeholder (500 px slot, 800w pick = density 1.6) must not shrink the
    // 100 px placeholder to 62.5 until the new image arrives. The new URLs are unreachable, so the
    // swap stays pending for the whole read.
    let config = EngineConfig::default();
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine
        .load_html(view, &page(format!("<img id=t src=\"{P100}\">")))
        .expect("load_html");
    let read = |engine: &mut Engine| -> f64 {
        let out = engine
            .execute_script(view, "String(document.getElementById('t').getBoundingClientRect().width)")
            .unwrap();
        out.strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&out)
            .parse()
            .unwrap_or(f64::NAN)
    };
    assert!((read(&mut engine) - 100.0).abs() < 0.5, "setup: the placeholder is 100 px");
    engine
        .execute_script(
            view,
            "var t = document.getElementById('t'); t.sizes = '500px'; \
             t.srcset = 'http://127.0.0.1:9/a.png 400w, http://127.0.0.1:9/b.png 800w';",
        )
        .unwrap();
    let w = read(&mut engine);
    assert!(
        (w - 100.0).abs() < 0.5,
        "the old 1x image is still painted, so it stays 100 px (not 100 / 1.6 = 62.5), got {w}"
    );
}

#[test]
fn a_resize_across_a_picture_breakpoint_fetches_the_new_source() {
    // Prometheus nit 2 on #1039: the pick depends on the viewport, so a resize across a `media`
    // breakpoint must request the source it now picks. The live turn plans images after any
    // layout that moved the frame generation, which a resize does.
    let png = P100
        .strip_prefix("data:image/png;base64,")
        .map(|b64| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).unwrap())
        .unwrap();
    let page = "<html><body style='margin:0'><picture>        <source media=\"(min-width: 600px)\" srcset=\"/wide.png\"><img id=t src=\"/narrow.png\">        </picture></body></html>";
    let server = crate::script_net_tests::serve_byte_routes(vec![
        ("/", "text/html", page.as_bytes().to_vec()),
        ("/narrow.png", "image/png", png.clone()),
        ("/wide.png", "image/png", png),
    ]);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    assert!(!server.served("/narrow.png").is_empty(), "setup: the narrow view fetched /narrow.png");
    assert!(server.served("/wide.png").is_empty(), "setup: /wide.png is not asked for at 400 px");
    engine
        .resize_view(view, Bounds { x: 0, y: 0, width: 800, height: 300 })
        .expect("resize");
    for _ in 0..40 {
        rt.block_on(engine.pump_live(view, 16));
        if !server.served("/wide.png").is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        !server.served("/wide.png").is_empty(),
        "after the resize past 600 px the picture picks /wide.png and it must be requested"
    );
}
