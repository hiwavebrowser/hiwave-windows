//! Compile-time checks of what may cross the page/window boundary
//! (`THREAD_OWNERSHIP_2026-10.md` §2.2, M3): the types a `FrameCommit`
//! will carry and the loaders the design shares by `Arc`. The design
//! checked them on Linux only; this file is compiled wherever the engine
//! tests build, so a macOS-only `!Send` (a CoreText handle in the font
//! path, say) fails the build here instead of at M4.
//!
//! Nothing runs: a type that stops being `Send + Sync` is a compile error.

#![cfg(test)]

use std::sync::Arc;

fn assert_send_sync<T: Send + Sync>() {}

/// The display list is what a frame is made of; the commit holds an
/// `Arc` of it.
#[test]
fn a_display_list_can_cross_to_the_window_thread() {
    assert_send_sync::<rustkit_layout::DisplayList>();
    assert_send_sync::<Arc<rustkit_layout::DisplayList>>();
    assert_send_sync::<rustkit_layout::DisplayCommand>();
}

/// The loaders the design shares across page threads by `Arc`.
#[test]
fn the_shared_loaders_can_be_shared_across_threads() {
    assert_send_sync::<Arc<rustkit_net::ResourceLoader>>();
    assert_send_sync::<Arc<rustkit_layout::FontLoader>>();
    assert_send_sync::<Arc<rustkit_image::ImageManager>>();
}

/// A presented frame's record names images by URL and nothing else, so
/// the presenter's bookkeeping can live on the window thread.
#[test]
fn a_presented_frame_record_carries_nothing_page_owned() {
    assert_send_sync::<super::PresentedFrame>();
}

/// The commit itself (second M3 PR): what a page thread will send to the
/// window thread at M4. An `Rc`, a reference or a page-owned handle in
/// it is a compile error here.
#[test]
fn a_frame_commit_can_be_sent_to_the_window_thread() {
    assert_send_sync::<super::FrameCommit>();
    assert_send_sync::<super::InputMark>();
}
