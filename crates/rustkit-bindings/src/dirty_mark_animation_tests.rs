//! LAT2 paint-only animation frames: the dirty mark that separates a Web
//! Animations layer write from every other invalidation.
//!
//! The engine paints an animation frame without a layout only when the
//! pending bucket holds nothing beyond `mark_animation_layer`. The sole
//! companion (`paint_only_animation_tests`) is `feature = "headless"`; these
//! pins keep the contract on every `cargo test -p rustkit-bindings --lib`.

use super::{DirtyMark, DomBindings, DomDirty};
use rustkit_dom::Document;
use rustkit_js::JsRuntime;
use std::rc::Rc;

#[test]
fn an_animation_layer_mark_alone_is_not_beyond_the_layer() {
    let dirty = DirtyMark::default();
    dirty.mark_animation_layer(DomDirty::Layout, "animation");
    assert_eq!(dirty.get(), DomDirty::Layout);
    assert!(!dirty.beyond_animation_layer());
    assert!(dirty.written_since_layout());
    // LAT7: the layer write still names the forced-layout cause.
    assert_eq!(dirty.cause(), Some("animation"));
}

#[test]
fn an_animation_paint_mark_is_not_beyond_the_layer_and_not_a_layout_write() {
    // LAT7c: a paint-only layer write fills the bucket for the paint-only
    // frame but leaves offset reads clean.
    let dirty = DirtyMark::default();
    dirty.mark_animation_paint(DomDirty::Layout, super::LAYER_PAINT);
    assert_eq!(dirty.get(), DomDirty::Layout);
    assert!(!dirty.beyond_animation_layer());
    assert!(!dirty.written_since_layout());
    assert!(dirty.needs_layout_for("computed_style"));
    assert!(!dirty.needs_layout_for("rect"));
}

#[test]
fn a_script_or_embedder_mark_is_beyond_the_layer() {
    let dirty = DirtyMark::default();
    dirty.mark(DomDirty::Style, "style");
    assert!(dirty.beyond_animation_layer());
    assert_eq!(dirty.get(), DomDirty::Style);
    assert_eq!(dirty.cause(), Some("style"));

    let dirty = DirtyMark::default();
    dirty.mark_animation_layer(DomDirty::Layout, "animation");
    dirty.mark(DomDirty::Layout, "attribute");
    assert!(dirty.beyond_animation_layer());
    // The first write since the layout is the cause.
    assert_eq!(dirty.cause(), Some("animation"));
}

#[test]
fn take_clears_the_beyond_flag_with_the_bucket() {
    let dirty = DirtyMark::default();
    dirty.mark(DomDirty::Style, "class");
    assert_eq!(dirty.take(), DomDirty::Style);
    assert_eq!(dirty.get(), DomDirty::Clean);
    assert!(!dirty.beyond_animation_layer());
}

#[test]
fn mark_dirty_on_bindings_is_beyond_and_take_dirty_clears_it() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><body></body></html>").unwrap(),
    ))
    .unwrap();
    let _ = b.take_dirty();
    assert!(!b.dirty_beyond_animation_layer());

    b.mark_dirty(DomDirty::Layout);
    assert!(b.dirty_beyond_animation_layer());
    assert_eq!(b.take_dirty(), DomDirty::Layout);
    assert!(!b.dirty_beyond_animation_layer());
}
