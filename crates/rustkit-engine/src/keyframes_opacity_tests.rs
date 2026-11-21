//! The static frame of a box whose `animation` names `@keyframes` that set
//! `opacity` when the engine runs no animations (`disable_animations`, a
//! parity capture): it paints the frame the animation settles on (reddit's
//! interstitial disc, `opacity: .75` with an infinite animation, painted
//! solid at 1). With animations running (the default since S13b) the frame
//! follows the clock; see `css_animations_tests`.

use super::*;

/// The opacity the red box of `css` paints at; 0 when it does not paint.
fn red_fade(css: &str, body: &str) -> f32 {
    let html = format!(
        "<!doctype html><style>body{{margin:0}}\
         .r{{background:#f00;width:100px;height:100px}}{css}</style><body>{body}</body>"
    );
    let config = EngineConfig {
        disable_animations: true,
        ..EngineConfig::default()
    };
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    engine.load_html(view, &html).expect("load");
    let list = engine.views[&view].display_list.as_ref().expect("display list");
    let red = rustkit_css::Color { r: 255, g: 0, b: 0, a: 1.0 };
    let mut stack = vec![1.0f32];
    for command in &list.commands {
        match command {
            rustkit_layout::DisplayCommand::PushOpacity(a) => stack.push(*a),
            rustkit_layout::DisplayCommand::PopOpacity => {
                stack.pop();
            }
            rustkit_layout::DisplayCommand::SolidColor(color, _) if *color == red => {
                return stack.iter().product();
            }
            _ => {}
        }
    }
    0.0
}

fn box_fade(css: &str) -> f32 {
    red_fade(css, r#"<div class="r"></div>"#)
}

fn assert_fade(got: f32, want: f32, case: &str) {
    assert!((got - want).abs() < 0.001, "{case}: painted at {got}, want {want}");
}

#[test]
fn reddits_disc_keeps_its_own_opacity_under_an_infinite_animation() {
    let got = red_fade(
        "main{position:relative;display:block;width:300px;height:300px}\
         main:before{animation:s 1.5s infinite ease-in-out;background:#f00;content:'';\
         height:100px;width:100px;opacity:.75;position:absolute}\
         @keyframes s{0%{transform:scale(1)}100%{transform:scale(1.5);opacity:0}}",
        "<main></main>",
    );
    assert_fade(got, 0.75, "infinite, 0% sets no opacity");
}

#[test]
fn a_forwards_animation_settles_on_its_last_keyframe() {
    let got = box_fade(".r{opacity:.75;animation:x 1s forwards}@keyframes x{100%{opacity:.25}}");
    assert_fade(got, 0.25, "forwards");
    let got = box_fade(".r{opacity:.75;animation:x 1s both}@keyframes x{to{opacity:.25}}");
    assert_fade(got, 0.25, "both, to");
}

#[test]
fn the_fade_in_idiom_stays_visible() {
    for fill in ["forwards", "both"] {
        let got = box_fade(&format!(
            ".r{{opacity:0;animation:fadeIn 1s {fill}}}\
             @keyframes fadeIn{{from{{opacity:0}}to{{opacity:1}}}}"
        ));
        assert_fade(got, 1.0, fill);
    }
}

/// With no forward fill a finite animation ends on the box's own value, so
/// a fade-in on a page section must not stay on its 0% frame for good.
#[test]
fn a_finite_animation_without_a_forward_fill_settles_on_the_box() {
    let got = box_fade(".r{animation:in .5s ease-in}@keyframes in{from{opacity:0}to{opacity:1}}");
    assert_fade(got, 1.0, "own opacity 1");
    let got = box_fade(".r{opacity:.5;animation:in .5s}@keyframes in{from{opacity:0}to{opacity:1}}");
    assert_fade(got, 0.5, "own opacity .5");
}

#[test]
fn an_infinite_animation_paints_the_first_keyframe_when_it_sets_opacity() {
    let got = box_fade(".r{animation:p 1s infinite}@keyframes p{0%,100%{opacity:.2}50%{opacity:1}}");
    assert_fade(got, 0.2, "0% in a selector list");
}

#[test]
fn keyframes_that_never_set_opacity_leave_the_box_alone() {
    let got = box_fade(".r{opacity:.5;animation:m 1s forwards}@keyframes m{to{transform:scale(2)}}");
    assert_fade(got, 0.5, "forwards");
}

#[test]
fn the_direction_picks_the_frame_it_ends_on() {
    let frames = "@keyframes d{from{opacity:.25}to{opacity:.75}}";
    let cases = [
        ("d 1s reverse forwards", 0.25),
        ("d 1s 2 alternate forwards", 0.25),
        ("d 1s 3 alternate forwards", 0.75),
        ("d 1s 2 alternate-reverse forwards", 0.75),
        ("d 1s infinite reverse", 0.75),
    ];
    for (animation, want) in cases {
        assert_fade(box_fade(&format!(".r{{animation:{animation}}}{frames}")), want, animation);
    }
}

#[test]
fn the_longhands_say_the_same_as_the_shorthand() {
    let got = box_fade(
        ".r{opacity:.75;animation-name:x;animation-duration:1s;animation-fill-mode:forwards}\
         @keyframes x{100%{opacity:.25}}",
    );
    assert_fade(got, 0.25, "longhands");
}

/// A later `animation` replaces the whole of an earlier one.
#[test]
fn a_later_shorthand_resets_the_earlier_fill() {
    let got = box_fade(
        ".r{opacity:.75;animation:x 1s forwards}.r{animation:x 1s}@keyframes x{100%{opacity:.25}}",
    );
    assert_fade(got, 0.75, "second shorthand has no fill");
}

#[test]
fn prefixed_nested_and_later_keyframes_count() {
    let got = box_fade(".r{opacity:.75;animation:x 1s forwards}@-webkit-keyframes x{to{opacity:.25}}");
    assert_fade(got, 0.25, "@-webkit-keyframes");
    let got = box_fade(
        ".r{opacity:.75;animation:x 1s forwards}@media screen{@keyframes x{to{opacity:.25}}}",
    );
    assert_fade(got, 0.25, "inside @media");
    let got = box_fade(
        ".r{opacity:.75;animation:x 1s forwards}\
         @keyframes x{to{opacity:.25}}@keyframes x{to{opacity:.5}}",
    );
    assert_fade(got, 0.5, "the last @keyframes of a name wins");
}

/// What the engine cannot read stays at today's guard (full opacity), never
/// invisible: a rule it failed to carry must not hide the page.
#[test]
fn an_animation_it_cannot_settle_still_paints_at_full_opacity() {
    let got = box_fade(".r{opacity:0;animation:nosuch 1s forwards}");
    assert_fade(got, 1.0, "no @keyframes of that name");
    let got = box_fade(
        ".r{opacity:0;animation:a 1s forwards,b 1s}@keyframes a{to{opacity:.25}}@keyframes b{}",
    );
    assert_fade(got, 1.0, "several animations");
    let got = box_fade(".r{opacity:0;animation:v 1s forwards}@keyframes v{to{opacity:var(--o)}}");
    assert_fade(got, 1.0, "a value it cannot read");
}

#[test]
fn a_box_with_no_animation_is_untouched() {
    assert_fade(box_fade(".r{opacity:.5}@keyframes x{to{opacity:.25}}"), 0.5, "no animation");
    assert_fade(box_fade(".r{opacity:.5;animation:none}"), 0.5, "animation: none");
}
