//! Portable pins for `display: inherit` (#817).
//!
//! The page oracles in `display_inherit_tests` need `feature = "headless"`,
//! so default Linux `--lib` never saw the apple-nav contents path or the
//! display-modifier copy. These call `apply_cascaded` directly.

use super::*;

fn parent_with(display: &str) -> ComputedStyle {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut parent = ComputedStyle::default();
    engine
        .lctx
        .apply_style_property(&mut parent, "display", display);
    parent
}

fn inherit_display(parent: &ComputedStyle) -> ComputedStyle {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::default();
    // Seed a different display so a no-op inherit is visible.
    engine
        .lctx
        .apply_style_property(&mut style, "display", "block");
    engine
        .lctx
        .apply_cascaded(&mut style, Some(parent), "display", "inherit");
    style
}

#[test]
fn display_inherit_copies_flex_and_leaves_block_seed_behind() {
    let got = inherit_display(&parent_with("flex"));
    assert_eq!(got.display, rustkit_css::Display::Flex);
    assert!(!got.display_contents_declared);
    assert_eq!(got.display_modifier, rustkit_css::DisplayModifier::None);
}

#[test]
fn display_inherit_under_contents_reads_contents_and_the_flag() {
    let parent = parent_with("contents");
    assert!(parent.display_contents_declared);
    let got = inherit_display(&parent);
    assert_eq!(got.display, rustkit_css::Display::Contents);
    assert!(got.display_contents_declared);
}

#[test]
fn display_inherit_copies_list_item_and_flow_root_modifiers() {
    let list = inherit_display(&parent_with("list-item"));
    assert_eq!(list.display, rustkit_css::Display::Block);
    assert_eq!(list.display_modifier, rustkit_css::DisplayModifier::ListItem);
    assert_eq!(list.display_keyword(), "list-item");

    let flow = inherit_display(&parent_with("flow-root"));
    assert_eq!(flow.display, rustkit_css::Display::Block);
    assert_eq!(flow.display_modifier, rustkit_css::DisplayModifier::FlowRoot);
    assert_eq!(flow.display_keyword(), "flow-root");
}

#[test]
fn display_inherit_copies_webkit_box() {
    let parent = parent_with("-webkit-box");
    assert_eq!(parent.webkit_box, Some(false));
    let got = inherit_display(&parent);
    assert_eq!(got.webkit_box, Some(false));
    assert_eq!(got.display_keyword(), "-webkit-box");
}
