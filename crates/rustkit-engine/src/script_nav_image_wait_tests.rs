//! A page that has asked to leave is not kept waiting for its images.
//!
//! Real window, 2026-10-12 (`i0a_nav`): a page whose first script set
//! `location.href` stayed for 4.9 s, 3 s of it waiting for an image a later
//! script had added. The app takes the page's navigation only when the load
//! has ended, the load runs on the window's thread, and the image belongs to
//! a page nobody will see.

use super::*;
use crate::script_net_tests::serve_routes;

/// The first script asks to leave; the second adds an image the server
/// holds (`/slow...`, 300 ms).
const PAGE: &str = "<html><body><p>leaving</p>
<script>location.href = '/next';</script>
<script>var img = document.createElement('img'); img.src = '/slow-added.png'; document.body.appendChild(img);</script>
</body></html>";

/// Adds the same image and stays.
const STAYS: &str = "<html><body><p>staying</p>
<script>var img = document.createElement('img'); img.src = '/slow-added.png'; document.body.appendChild(img);</script>
</body></html>";

/// Loads `page` and returns the requests the server got and where the
/// page's script asked to go.
fn load(config: EngineConfig, page: &str) -> (usize, Option<String>) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let asked = engine.take_script_navigation(view);
    (server.hits.load(std::sync::atomic::Ordering::SeqCst), asked)
}

fn follows() -> EngineConfig {
    EngineConfig { follows_script_navigation: true, ..EngineConfig::default() }
}

#[test]
fn a_page_that_asked_to_leave_does_not_wait_for_an_image_its_script_added() {
    let (requests, asked) = load(follows(), PAGE);
    assert_eq!(requests, 1, "the page alone was requested");
    // The request is still there for the embedder to take.
    assert!(asked.as_deref().is_some_and(|url| url.ends_with("/next")), "{asked:?}");
}

#[test]
fn a_page_that_stays_still_gets_the_image_its_script_added() {
    let (requests, asked) = load(follows(), STAYS);
    assert_eq!(requests, 2, "the page and its image");
    assert_eq!(asked, None);
}

/// A form the page's script submits may be one the embedder does not
/// follow (only GET is wired), so the page may stay: it keeps its images.
#[test]
fn a_page_whose_script_submitted_a_form_still_gets_the_image() {
    let page = "<html><body><form id=f method=post action=/next><input name=q value=1></form>
<script>document.getElementById('f').submit();</script>
<script>var img = document.createElement('img'); img.src = '/slow-added.png'; document.body.appendChild(img);</script>
</body></html>";
    let (requests, asked) = load(follows(), page);
    assert_eq!(requests, 2, "the page and its image");
    assert_eq!(asked, None, "a POST is not followed");
}

/// A capture does not follow the page: it shows the page as it was, with
/// what its scripts added.
#[test]
fn without_the_switch_the_load_is_as_before() {
    let (requests, asked) = load(EngineConfig::default(), PAGE);
    assert_eq!(requests, 2, "the page and its image");
    assert!(asked.as_deref().is_some_and(|url| url.ends_with("/next")), "{asked:?}");
}
