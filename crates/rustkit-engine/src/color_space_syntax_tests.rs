//! CSS Color 4 space syntax for `rgb()` / `hsl()` — `rgb(70 31 140 / 60%)`,
//! what Tailwind v3 writes for every ring, border and shadow colour — through
//! the cascade to `getComputedStyle` and to paint, in each property that takes
//! a colour. The shared parser took only the comma form, so these were all
//! dropped (or, in a shorthand, kept with the colour left as currentcolor).
//! Every expected value is Chromium 143.0.7499.4's answer on the same page
//! (400x300, a hand-made page; pixels read from its screenshot).

use super::*;
use rustkit_css::{Color, Length};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;

fn loaded(body: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: WIDTH,
            height: HEIGHT,
        })
        .expect("view");
    let html = format!(
        "<!doctype html><style>html,body{{margin:0;background:#fff}}div{{box-sizing:border-box}}</style>{body}"
    );
    engine.load_html(view, &html).expect("load_html");
    (engine, view)
}

fn eval(engine: &mut Engine, view: EngineViewId, script: &str) -> String {
    let out = engine
        .execute_script(view, &format!("String(eval({script:?}))"))
        .expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .unwrap_or_else(|| panic!("{script} evaluated to {out}"))
        .replace("\\\"", "\"")
}

fn frame(engine: &mut Engine, view: EngineViewId) -> Vec<u8> {
    engine.render_view(view).expect("render");
    let path = std::env::temp_dir().join(format!(
        "rustkit-color-space-{}-{:?}.ppm",
        std::process::id(),
        std::thread::current().id()
    ));
    engine
        .capture_frame(view, path.to_str().unwrap())
        .expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm
}

fn pixel(ppm: &[u8], x: usize, y: usize) -> [u8; 3] {
    let p = &ppm[ppm.len() - WIDTH as usize * HEIGHT as usize * 3..];
    let at = (y * WIDTH as usize + x) * 3;
    [p[at], p[at + 1], p[at + 2]]
}

const SEED: Color = Color {
    r: 9,
    g: 9,
    b: 9,
    a: 1.0,
};

/// One declaration applied over a style whose colours are all `SEED`, so a
/// dropped declaration shows as `SEED` left in place.
fn applied(property: &str, value: &str) -> ComputedStyle {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::new();
    style.color = SEED;
    style.background_color = SEED;
    style.border_top_color = SEED;
    style.outline.color = Some(SEED);
    engine
        .lctx
        .apply_style_property(&mut style, property, value);
    style
}

/// Each colour as Chromium computes it, through every property that takes
/// one. `None` is an invalid colour: Chromium drops the declaration.
fn check_matrix(cases: &[(&str, Option<Color>)]) {
    let mut wrong = Vec::new();
    for (input, want) in cases {
        let kept = want.unwrap_or(SEED);
        let stop = |s: &ComputedStyle| match &s.background_gradient {
            Some(rustkit_css::Gradient::Linear(g)) => g.stops.first().map(|s| s.color),
            _ => None,
        };
        let rows: [(&str, String, Option<Color>, Option<Color>); 8] = [
            (
                "color",
                input.to_string(),
                Some(applied("color", input).color),
                Some(kept),
            ),
            (
                "background-color",
                input.to_string(),
                Some(applied("background-color", input).background_color),
                Some(kept),
            ),
            (
                "border-color",
                input.to_string(),
                Some(applied("border-color", input).border_top_color),
                Some(kept),
            ),
            (
                "border",
                format!("4px solid {input}"),
                Some(applied("border", &format!("4px solid {input}")).border_top_color),
                Some(kept),
            ),
            (
                "outline-color",
                input.to_string(),
                applied("outline-color", input).outline.color,
                Some(kept),
            ),
            (
                "outline",
                format!("2px solid {input}"),
                applied("outline", &format!("2px solid {input}"))
                    .outline
                    .color,
                Some(kept),
            ),
            (
                "box-shadow",
                format!("0 0 0 3px {input}"),
                applied("box-shadow", &format!("0 0 0 3px {input}"))
                    .box_shadows
                    .first()
                    .map(|s| s.color),
                *want,
            ),
            (
                "background-image",
                format!("linear-gradient({input}, {input})"),
                stop(&applied(
                    "background-image",
                    &format!("linear-gradient({input}, {input})"),
                )),
                *want,
            ),
        ];
        for (prop, value, got, want) in rows {
            if got != want {
                wrong.push(format!("{prop}: {value}: got {got:?}, want {want:?}"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

fn rgb(r: u8, g: u8, b: u8) -> Option<Color> {
    Some(Color::from_rgb(r, g, b))
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Option<Color> {
    Some(Color::new(r, g, b, a))
}

#[test]
fn space_syntax_reaches_every_colour_property() {
    check_matrix(&[
        ("rgb(24 25 26)", rgb(24, 25, 26)),
        ("rgb(70 31 140 / 60%)", rgba(70, 31, 140, 0.6)),
        ("rgb(0 0 0 / .1)", rgba(0, 0, 0, 0.1)),
        ("hsl(210 40% 50% / 0.5)", rgba(77, 128, 179, 0.5)),
        ("rgb(100% 50% 0%)", rgb(255, 128, 0)),
        ("rgb(none 128 255)", rgb(0, 128, 255)),
    ]);
}

#[test]
fn mixed_comma_and_space_syntax_is_dropped() {
    check_matrix(&[("rgb(1, 2 3)", None), ("rgb(1 2 3, 0.5)", None)]);
}

/// Chromium drops the whole shorthand over a bad colour: `border: 7px solid
/// rgb(1, 2 3)` leaves the width at its initial 0px (computed, style none),
/// and the shadow at `none`.
#[test]
fn a_mixed_form_colour_drops_the_whole_shorthand() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut wrong = Vec::new();
    for bad in ["rgb(1, 2 3)", "rgb(1 2 3, 0.5)", "hsl(1, 2% 3%)"] {
        let mut style = ComputedStyle::new();
        style.border_top_width = Length::Px(1.0);
        engine
            .lctx
            .apply_style_property(&mut style, "border", &format!("7px solid {bad}"));
        if style.border_top_width != Length::Px(1.0) {
            wrong.push(format!(
                "border: 7px solid {bad}: width became {:?}",
                style.border_top_width
            ));
        }
        let mut style = ComputedStyle::new();
        engine
            .lctx
            .apply_style_property(&mut style, "box-shadow", &format!("0 0 0 3px {bad}"));
        if !style.box_shadows.is_empty() {
            wrong.push(format!(
                "box-shadow: 0 0 0 3px {bad}: kept {:?}",
                style.box_shadows
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn the_legacy_comma_form_still_reaches_every_property() {
    check_matrix(&[
        ("rgb(1, 2, 3)", rgb(1, 2, 3)),
        ("rgba(1, 2, 3, 0.5)", rgba(1, 2, 3, 0.5)),
        ("hsl(210, 40%, 50%)", rgb(77, 128, 179)),
    ]);
}

/// `getComputedStyle` reads of a resolved colour: Chromium's serialisation.
#[test]
fn computed_reads_serialise_the_resolved_colour() {
    let (mut engine, view) = loaded(
        r#"<div id=a style="outline:2px solid rgb(70 31 140 / 60%)"></div><div id=b style="outline:2px solid hsl(210 40% 50%)"></div>"#,
    );
    assert_eq!(
        eval(
            &mut engine,
            view,
            "['a','b'].map(function(i){return getComputedStyle(document.getElementById(i)).outlineColor}).join(' | ')"
        ),
        "rgba(70, 31, 140, 0.6) | rgb(77, 128, 179)"
    );
}

/// Painted pixels: each box is 60x40 at 20,20 on white.
#[test]
fn space_syntax_colours_paint() {
    let boxed = |style: &str| {
        format!("<div style=\"position:absolute;left:20px;top:20px;width:60px;height:40px;{style}\"></div>")
    };
    // (style, point, Chromium's pixel there)
    let cases: [(&str, (usize, usize), [u8; 3]); 9] = [
        (
            "background-color:rgb(255 0 0 / 0.5)",
            (50, 40),
            [255, 127, 127],
        ),
        (
            "border:10px solid rgb(0 0 255 / 50%)",
            (25, 40),
            [127, 127, 255],
        ),
        (
            "outline:6px solid rgb(70 31 140 / 60%)",
            (17, 40),
            [144, 121, 186],
        ),
        (
            "box-shadow:0 0 0 8px hsl(120 100% 25% / 0.5)",
            (15, 40),
            [127, 191, 127],
        ),
        (
            "background-image:linear-gradient(rgb(0 0 255), rgb(0 0 255))",
            (50, 40),
            [0, 0, 255],
        ),
        // `color` reaches paint through the outline's currentcolor. (Not a
        // colourless `border`: that keeps the old border colour rather than
        // currentcolor in any syntax, a separate gap.)
        (
            "outline:6px solid;color:rgb(0 128 0)",
            (17, 40),
            [0, 128, 0],
        ),
        (
            "background-color:blue;background-color:rgb(255, 0 0)",
            (50, 40),
            [0, 0, 255],
        ),
        (
            "background-color:rgb(none 128 255)",
            (50, 40),
            [0, 128, 255],
        ),
        (
            "background-color:rgb(calc(100 + 155) 0 0)",
            (50, 40),
            [255, 0, 0],
        ),
    ];
    let mut wrong = Vec::new();
    for (style, (x, y), want) in cases {
        let (mut engine, view) = loaded(&boxed(style));
        let got = pixel(&frame(&mut engine, view), x, y);
        // Blending may land one step either side of Chromium's rounding.
        if got.iter().zip(want).any(|(g, w)| g.abs_diff(w) > 1) {
            wrong.push(format!("{style}: at {x},{y} got {got:?}, want {want:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// R1 on #870: an `rgb()`/`hsl()` Chromium accepts but RustKit cannot read
/// yet (relative colour, a math function it does not evaluate) is a valid
/// colour, so the shorthand keeps its other parts as on develop; only a
/// grammar error the parser is sure of drops it.
#[test]
fn an_unreadable_but_valid_colour_function_keeps_the_shorthand() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut wrong = Vec::new();
    for value in [
        "rgb(from red r g b)",
        "rgb(from red r g b / 50%)",
        "hsl(from red h s l)",
        "rgb(mod(5, 3) 0 0)",
        "rgb(sin(1) 0 0)",
    ] {
        let mut style = ComputedStyle::new();
        engine.lctx.apply_style_property(&mut style, "border", "4px dashed red");
        engine.lctx.apply_style_property(&mut style, "border", &format!("1px solid {value}"));
        if style.border_top_width != Length::Px(1.0) {
            wrong.push(format!("border: 1px solid {value}: width {:?}", style.border_top_width));
        }
        let mut style = ComputedStyle::new();
        engine.lctx.apply_style_property(&mut style, "background", "url(a.png) red");
        engine.lctx.apply_style_property(&mut style, "background", &format!("url(x.png) {value}"));
        let image = style.background_layers.first().map(|l| format!("{:?}", l.image));
        if !image.as_deref().is_some_and(|i| i.contains("x.png")) {
            wrong.push(format!("background: url(x.png) {value}: image {image:?}"));
        }
        let mut style = ComputedStyle::new();
        engine.lctx.apply_style_property(&mut style, "box-shadow", &format!("1px 2px 3px {value}"));
        if style.box_shadows.len() != 1 {
            wrong.push(format!("box-shadow: 1px 2px 3px {value}: {} shadows", style.box_shadows.len()));
        }
    }
    assert!(wrong.is_empty(), "{} wrong:\n{}", wrong.len(), wrong.join("\n"));
}

/// R1's painted examples, on a 60x60 box at 20,20. Chromium paints red at
/// each point: the relative colour resolves there; here the border and the
/// shadow are kept (colour not resolved yet), and `min()` resolves.
#[test]
fn the_r1_painted_examples() {
    let paint = |html: &str, x: usize, y: usize| {
        let (mut engine, view) = loaded(html);
        pixel(&frame(&mut engine, view), x, y)
    };
    let boxed = |style: &str| {
        format!("<div style=\"position:absolute;left:20px;top:20px;width:60px;height:60px;{style}\"></div>")
    };
    assert_ne!(
        paint(&boxed("border:20px solid rgb(from red r g b)"), 25, 50),
        [255, 255, 255],
        "the border is kept"
    );
    assert_ne!(
        paint(&boxed("box-shadow:0 0 0 8px rgb(from red r g b)"), 15, 50),
        [255, 255, 255],
        "the shadow is kept"
    );
    assert_eq!(
        paint(
            "<style>#m{background:blue;background:url(nope.png) rgb(min(255, 300) 0 0)}</style><div id=m style=\"position:absolute;left:20px;top:20px;width:60px;height:60px\"></div>",
            50,
            50
        ),
        [255, 0, 0],
        "the later background wins, its min() resolved"
    );
}
