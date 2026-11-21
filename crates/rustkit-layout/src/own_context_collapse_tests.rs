//! Portable #863 collapse-through fence pins.
//!
//! `float_margin_tests` is headless-only. Linux `--lib` never saw which
//! boxes stop margin collapse-through (`flow-root`, contain, content-
//! visibility, container-type, declared writing-mode, and the tag fence).

use super::*;
use rustkit_css::{ContainerType, DisplayModifier, STACKS_CONTENT_VISIBILITY};

fn box_with(edit: impl FnOnce(&mut LayoutBox)) -> LayoutBox {
    let mut b = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    edit(&mut b);
    b
}

#[test]
fn style_flags_that_stop_collapse_through() {
    assert!(
        !box_with(|_| {}).is_own_context_for_collapse(),
        "a plain empty div collapses through"
    );
    assert!(box_with(|b| {
        b.style.display_modifier = DisplayModifier::FlowRoot;
    })
    .is_own_context_for_collapse());
    assert!(box_with(|b| {
        b.style.contain_holds_fixed = true;
    })
    .is_own_context_for_collapse());
    assert!(box_with(|b| {
        b.style.stacks |= STACKS_CONTENT_VISIBILITY;
    })
    .is_own_context_for_collapse());
    assert!(box_with(|b| {
        b.style.container_type = ContainerType::Size;
    })
    .is_own_context_for_collapse());
    assert!(box_with(|b| {
        b.style.writing_mode_declared = true;
    })
    .is_own_context_for_collapse());
}

#[test]
fn tag_fence_stops_collapse_and_a_div_does_not() {
    for tag in ["hr", "fieldset", "details", "img", "button", "input", "svg"] {
        let b = box_with(|b| {
            b.set_identity(ElementIdentity {
                element_id: 1,
                tag: tag.into(),
                selector: tag.into(),
            });
        });
        assert!(
            b.is_own_context_for_collapse(),
            "{tag} must stop collapse-through"
        );
    }
    let div = box_with(|b| {
        b.set_identity(ElementIdentity {
            element_id: 2,
            tag: "div".into(),
            selector: "div".into(),
        });
    });
    assert!(!div.is_own_context_for_collapse());
}
