//! CSS Color 4 §4 `opacity` at the display-list boundary.
//!
//! `opacity` was parsed into the style and read by nothing but the image
//! command (whose painter dropped it): a `div` at `opacity: 0` painted
//! solid. google.com's search box (2026-10-08) carries two such layers, a
//! conic-gradient glow and a black hover overlay, which Chrome does not
//! show and RustKit painted over the box
//! (docs/diagnostics/2026-10-08/google_searchbox_conic_h22.md).

use super::*;

fn red() -> Color {
    Color { r: 255, g: 0, b: 0, a: 1.0 }
}

fn blue() -> Color {
    Color { r: 0, g: 0, b: 255, a: 1.0 }
}

fn filled(color: Color, opacity: f32, rect: Rect) -> LayoutBox {
    let mut style = ComputedStyle::new();
    style.background_color = color;
    style.opacity = opacity;
    let mut b = LayoutBox::new(BoxType::Block, style);
    b.dimensions.content = rect;
    b
}

/// A white page holding one red 100x100 box at `opacity`, which holds a
/// blue 50x50 box.
fn page(opacity: f32) -> LayoutBox {
    let mut root = filled(Color::WHITE, 1.0, Rect::new(0.0, 0.0, 800.0, 600.0));
    let mut outer = filled(red(), opacity, Rect::new(10.0, 10.0, 100.0, 100.0));
    outer.children.push(filled(blue(), 1.0, Rect::new(10.0, 10.0, 50.0, 50.0)));
    root.children.push(outer);
    root
}

fn fills(list: &DisplayList, of: Color) -> usize {
    list.commands
        .iter()
        .filter(|c| match c {
            DisplayCommand::SolidColor(color, _) | DisplayCommand::RoundedRect { color, .. } => *color == of,
            _ => false,
        })
        .count()
}

#[test]
fn an_opaque_box_paints_as_before() {
    let list = DisplayList::build(&page(1.0));
    assert_eq!((fills(&list, red()), fills(&list, blue())), (1, 1), "{:?}", list.commands);
}

#[test]
fn a_box_at_opacity_zero_paints_nothing() {
    let list = DisplayList::build(&page(0.0));
    assert_eq!(fills(&list, red()), 0, "the box itself: {:?}", list.commands);
}

#[test]
fn the_subtree_of_a_box_at_opacity_zero_paints_nothing() {
    // The child's own opacity is 1: opacity is not inherited, the group is
    // faded as one (CSS Color 4 §4).
    let list = DisplayList::build(&page(0.0));
    assert_eq!(fills(&list, blue()), 0, "its child: {:?}", list.commands);
    assert_eq!(fills(&list, Color::WHITE), 1, "what is under it still paints");
}

/// The commands of `list` with the product of the opacity scopes each sits
/// in.
fn faded(list: &DisplayList) -> Vec<(&DisplayCommand, f32)> {
    let mut open: Vec<f32> = Vec::new();
    let mut out = Vec::new();
    for command in &list.commands {
        match command {
            DisplayCommand::PushOpacity(alpha) => open.push(*alpha),
            DisplayCommand::PopOpacity => {
                open.pop().expect("a pop for every push");
            }
            other => out.push((other, open.iter().product())),
        }
    }
    assert!(open.is_empty(), "every scope is closed: {:?}", list.commands);
    out
}

fn fade_of(list: &DisplayList, of: Color) -> f32 {
    faded(list)
        .into_iter()
        .find_map(|(c, alpha)| match c {
            DisplayCommand::SolidColor(color, _) if *color == of => Some(alpha),
            _ => None,
        })
        .expect("the fill")
}

#[test]
fn a_translucent_box_fades_itself_and_its_subtree() {
    let list = DisplayList::build(&page(0.5));
    assert_eq!(fade_of(&list, red()), 0.5);
    assert_eq!(fade_of(&list, blue()), 0.5, "the child has no opacity of its own");
    assert_eq!(fade_of(&list, Color::WHITE), 1.0, "what is outside is not faded");
}

#[test]
fn an_opaque_box_opens_no_scope() {
    let list = DisplayList::build(&page(1.0));
    assert!(
        !list
            .commands
            .iter()
            .any(|c| matches!(c, DisplayCommand::PushOpacity(_) | DisplayCommand::PopOpacity)),
        "{:?}",
        list.commands
    );
}

#[test]
fn nested_opacities_multiply() {
    let mut root = page(0.5);
    root.children[0].children[0].style.opacity = 0.5;
    let list = DisplayList::build(&root);
    assert_eq!(fade_of(&list, red()), 0.5);
    assert_eq!(fade_of(&list, blue()), 0.25);
}

#[test]
fn a_text_run_does_not_apply_the_opacity_it_copied_from_its_parent() {
    // A pseudo-element's text child clones the pseudo's whole style.
    let mut root = page(0.5);
    let mut style = root.children[0].style.clone();
    style.background_color = Color::TRANSPARENT;
    root.children[0].children.push(LayoutBox::new(BoxType::Text("x".into()), style));
    let list = DisplayList::build(&root);
    let pushes = list
        .commands
        .iter()
        .filter(|c| matches!(c, DisplayCommand::PushOpacity(_)))
        .count();
    assert_eq!(pushes, 1, "{:?}", list.commands);
}

/// Keyframe animations are parsed and not run. The fade-in idiom, `opacity:
/// 0; animation: appear 1s forwards`, would leave its content invisible for
/// good, so a box that names an animation paints at full opacity (as every
/// box did before opacity was painted at all) until animations run.
#[test]
fn a_box_with_a_keyframe_animation_is_not_faded_while_animations_do_not_run() {
    for opacity in [0.0, 0.5] {
        let mut root = page(opacity);
        root.children[0].style.animation_name = "appear".into();
        let list = DisplayList::build(&root);
        assert_eq!(fills(&list, red()), 1, "at {opacity}: {:?}", list.commands);
        assert_eq!(fade_of(&list, red()), 1.0, "at {opacity}");
    }
}

#[test]
fn animation_name_none_is_no_animation() {
    for name in ["", "none", "NONE"] {
        let mut root = page(0.0);
        root.children[0].style.animation_name = name.into();
        let list = DisplayList::build(&root);
        assert_eq!(fills(&list, red()), 0, "animation-name {name:?}");
    }
}

#[test]
fn the_scope_opens_before_the_boxs_clip_and_closes_after_it() {
    // Pushes and pops stay nested.
    let mut root = page(0.5);
    root.children[0].style.overflow_x = rustkit_css::Overflow::Hidden;
    root.children[0].style.overflow_y = rustkit_css::Overflow::Hidden;
    let list = DisplayList::build(&root);
    let at = |want: fn(&DisplayCommand) -> bool| list.commands.iter().position(want).expect("command");
    let push = at(|c| matches!(c, DisplayCommand::PushOpacity(_)));
    let clip = at(|c| matches!(c, DisplayCommand::PushClip(_)));
    let unclip = at(|c| matches!(c, DisplayCommand::PopClip));
    let pop = at(|c| matches!(c, DisplayCommand::PopOpacity));
    assert!(push < clip && clip < unclip && unclip < pop, "{:?}", list.commands);
}
