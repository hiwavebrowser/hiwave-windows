use super::*;
use rustkit_core::{InputEvent, MouseButton, MouseEvent, MouseEventType, Point};

fn page(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).unwrap();
    let id = engine.create_headless_view(Bounds::new(0, 0, 400, 200)).unwrap();
    engine.load_html(id, html).unwrap();
    (engine, id)
}

fn pointer(engine: &mut Engine, id: EngineViewId, kind: MouseEventType, button: MouseButton) -> Option<(EngineViewId, String)> {
    let view_id = engine.views[&id].viewhost_id;
    engine.handle_view_event(rustkit_viewhost::ViewEvent::Input {
        view_id,
        event: InputEvent::Mouse(MouseEvent::new(kind, Point::new(20.0, 20.0)).with_button(button)),
    })
}

#[test]
fn native_click_dispatches_script_and_honors_cancelled_navigation() {
    let (mut engine, id) = page(r#"<style>body{margin:0}a{display:block;width:200px;height:60px}</style>
        <a id="link" href="https://example.com/next">link</a>"#);
    engine.execute_script(id, "globalThis.clicks=0; document.getElementById('link').addEventListener('click',e=>{clicks++;e.preventDefault()})").unwrap();
    pointer(&mut engine, id, MouseEventType::MouseDown, MouseButton::Primary);
    assert!(pointer(&mut engine, id, MouseEventType::MouseUp, MouseButton::Primary).is_none());
    assert_eq!(engine.execute_script(id, "String(clicks)").unwrap(), "String(\"1\")");
}

#[test]
fn native_primary_click_returns_default_navigation_but_secondary_does_not() {
    let (mut engine, id) = page(r#"<style>body{margin:0}a{display:block;width:200px;height:60px}</style>
        <a href="https://example.com/next">link</a>"#);
    pointer(&mut engine, id, MouseEventType::MouseDown, MouseButton::Secondary);
    assert!(pointer(&mut engine, id, MouseEventType::MouseUp, MouseButton::Secondary).is_none());
    pointer(&mut engine, id, MouseEventType::MouseDown, MouseButton::Primary);
    assert_eq!(pointer(&mut engine, id, MouseEventType::MouseUp, MouseButton::Primary),
        Some((id, "https://example.com/next".into())));
}

#[test]
fn native_wheel_moves_the_document_and_notifies_script() {
    let (mut engine, id) = page("<style>body{margin:0;height:2000px}</style><p>scroll me</p>");
    let view_id = engine.views[&id].viewhost_id;
    engine.handle_view_event(rustkit_viewhost::ViewEvent::Input {
        view_id,
        event: InputEvent::Mouse(MouseEvent::new(MouseEventType::Wheel, Point::new(20.0, 20.0))
            .with_delta(Point::new(0.0, -1.0))),
    });
    assert_eq!(engine.views[&id].scroll_offset.1, 120.0);
    assert_eq!(engine.execute_script(id, "String(scrollY)").unwrap(), "String(\"120\")");
}
