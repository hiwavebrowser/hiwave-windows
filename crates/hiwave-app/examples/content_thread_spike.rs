//! Spike: can a window host the engine's content thread on Windows?
//!
//! Opens a window, attaches a `WindowSurface` (the window thread's half), spawns
//! a `ContentThread` (the page's half), loads a URL, and presents the frames the
//! page publishes. No input, no tabs, no chrome: just enough to measure whether
//! the window stays responsive while a heavy page loads (the native shell today
//! loads on the window thread and freezes).
//!
//! ```text
//! cargo run --release -p hiwave-app --example content_thread_spike \
//!     --no-default-features --features native-win32 -- https://cnn.com
//! ```
#![cfg(windows)]

use std::time::{Duration, Instant};

use rustkit_engine::{ContentThread, EngineBuilder, FrameCommit, LoadState, PageEvent, WindowSurface};
use rustkit_viewhost::{Bounds, MainWindowConfig, ViewHost};
use url::Url;

fn main() {
    let url = std::env::args().nth(1).unwrap_or_else(|| "https://example.com".to_string());
    let url = Url::parse(&url).expect("a URL");

    let host = ViewHost::new();
    let hwnd = host
        .create_main_window(MainWindowConfig::new("content thread spike").with_size(1280, 800))
        .expect("main window");

    // The client area is smaller than the outer size by the frame.
    let bounds = client_bounds(hwnd).unwrap_or_else(|| Bounds::new(0, 0, 1264, 761));

    // The window thread's half: a host view and a GPU surface, created HERE.
    let mut surface = WindowSurface::attach(hwnd, bounds).expect("window surface");

    // The page's half: the engine is built ON the content thread.
    let builder = EngineBuilder::new()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) HiWave-spike")
        .javascript_enabled(true)
        .script_budget_ms(5_000)
        .interrupt_scripts_at_budget(true);
    let thread = ContentThread::spawn(builder, bounds).expect("content thread");

    let started = Instant::now();
    let nav = thread.navigate(url.clone());
    println!("navigate {url} nav_id={nav}");

    let mut shown = 0u64;
    let mut first_commit = None;
    loop {
        if !host.pump_messages() {
            break;
        }

        let mut latest: Option<FrameCommit> = None;
        for event in thread.poll() {
            match event {
                PageEvent::Commit(commit) => latest = Some(commit),
                PageEvent::Title { text, .. } => println!("{:>6} ms  title {text:?}", started.elapsed().as_millis()),
                PageEvent::LoadState { state, .. } => {
                    let label = match &state {
                        LoadState::Started => "started".to_string(),
                        LoadState::Complete => "complete".to_string(),
                        LoadState::Failed { error } => format!("failed: {error}"),
                    };
                    println!("{:>6} ms  load {label}", started.elapsed().as_millis());
                }
                _ => {}
            }
        }
        if let (Some(commit), Some(images)) = (latest, thread.images()) {
            first_commit.get_or_insert_with(|| started.elapsed());
            match surface.present_commit(&commit, &images) {
                Ok(()) => shown += 1,
                Err(e) => eprintln!("present failed: {e}"),
            }
        }

        std::thread::sleep(Duration::from_millis(8));
    }
    println!("presented {shown} frames; first at {:?}", first_commit);
}

fn client_bounds(hwnd: windows::Win32::Foundation::HWND) -> Option<Bounds> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    let mut rect = RECT::default();
    // SAFETY: `hwnd` is the live window just created; `rect` is a valid out-pointer.
    unsafe { GetClientRect(hwnd, &mut rect) }.ok()?;
    let (w, h) = ((rect.right - rect.left).max(0) as u32, (rect.bottom - rect.top).max(0) as u32);
    (w > 0 && h > 0).then(|| Bounds::new(0, 0, w, h))
}
