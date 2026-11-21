//! HTML drag and drop (HTML §6.11.5, the drag-and-drop processing model)
//! over the engine's pointer entry points.
//!
//! A press that the page did not cancel, on an element that is draggable
//! (`draggable="true"`, or by default an `<a href>` or an `<img>`), is a
//! drag candidate. A move of more than [`DRAG_THRESHOLD`] px with the button
//! held fires `dragstart` at the source; uncancelled, the drag begins
//! (`pointercancel` at the source, Pointer Events §11.4) and each move runs
//! one iteration of the processing model: `drag` at the source, then
//! `dragenter` at the element under the pointer when it changed and
//! `dragleave` at the one it left, then `dragover` there. A cancelled
//! `dragover` makes that element a drop target with the operation its
//! `dataTransfer` allows; an uncancelled one is "no drop". The release fires
//! `drop` at a valid target (or `dragleave` at an invalid one), then
//! `dragend` at the source with the final `dropEffect`. While the drag runs
//! the page hears no mouse or pointer move, no `mouseup` and no `click`;
//! `:hover` still follows the pointer.
//!
//! The data store and its protection modes live in script
//! (rustkit-bindings web_dnd.js); this side keeps the drag's elements and
//! its current operation.

use super::*;

/// How far (CSS px, either axis) the pointer moves with the button held
/// before a press on a draggable element becomes a drag.
pub(crate) const DRAG_THRESHOLD: f32 = 3.0;

/// The drag state of one view.
#[derive(Debug, Default)]
pub(crate) struct DragState {
    /// A press on a draggable element: the source (raw NodeId) and where
    /// the press was, in viewport coordinates.
    candidate: Option<(usize, f32, f32)>,
    /// The drag in progress.
    active: Option<ActiveDrag>,
}

#[derive(Debug)]
struct ActiveDrag {
    /// The element being dragged (raw NodeId).
    source: usize,
    /// The element under the pointer that last heard `dragenter` (raw NodeId).
    current: Option<usize>,
    /// The current drag operation: `none`, `copy`, `link` or `move`.
    operation: String,
}

impl DragState {
    pub(crate) fn dragging(&self) -> bool {
        self.active.is_some()
    }
}

impl Engine {
    /// The element a press on `target` drags: the nearest one at or above
    /// it that is draggable, unless a `draggable="false"` comes first.
    fn drag_source(&self, id: EngineViewId, target: usize) -> Option<usize> {
        let mut node = self.node(id, target);
        while let Some(n) = node {
            if n.is_element() {
                match n
                    .get_attribute("draggable")
                    .map(|v| v.trim().to_ascii_lowercase())
                {
                    Some(v) if v == "true" => return Some(n.id.raw()),
                    Some(v) if v == "false" => return None,
                    _ => {}
                }
                let tag = n.tag_name().map(|t| t.to_ascii_lowercase());
                match tag.as_deref() {
                    Some("img") => return Some(n.id.raw()),
                    Some("a") if n.get_attribute("href").is_some() => return Some(n.id.raw()),
                    // A press in a text control or an editing host selects
                    // text there, as in Chrome: it does not drag the row
                    // around it.
                    Some("input" | "textarea" | "select") => return None,
                    _ => {}
                }
                let editable = n
                    .get_attribute("contenteditable")
                    .map(|v| v.trim().to_ascii_lowercase());
                if matches!(editable.as_deref(), Some("" | "true" | "plaintext-only")) {
                    return None;
                }
            }
            node = n.parent();
        }
        None
    }

    /// After a press: remember a drag candidate when the press was not
    /// cancelled and landed on something draggable (and not on a disabled
    /// form control).
    pub(crate) fn note_drag_candidate(
        &mut self,
        id: EngineViewId,
        target: Option<usize>,
        pressed: bool,
        viewport_x: f32,
        viewport_y: f32,
    ) {
        let candidate = target
            .filter(|_| pressed)
            .filter(|t| !self.node(id, *t).is_some_and(Self::in_disabled_control))
            .and_then(|t| self.drag_source(id, t))
            .map(|source| (source, viewport_x, viewport_y));
        if let Some(view) = self.views.get_mut(&id) {
            view.drag = DragState {
                candidate,
                active: None,
            };
        }
    }

    /// A pointer move: start the drag when a candidate's pointer went past
    /// the threshold, then run an iteration of the drag. Returns true when
    /// the move belonged to a drag (and no mouse event is due).
    pub(crate) fn drag_move(&mut self, id: EngineViewId, viewport_x: f32, viewport_y: f32) -> bool {
        let Some(view) = self.views.get_mut(&id) else {
            return false;
        };
        if view.drag.active.is_none() {
            let Some((source, x0, y0)) = view.drag.candidate else {
                return false;
            };
            if !view.primary_button_down {
                view.drag.candidate = None;
                return false;
            }
            if (viewport_x - x0).abs() <= DRAG_THRESHOLD
                && (viewport_y - y0).abs() <= DRAG_THRESHOLD
            {
                return false;
            }
            // One `dragstart` per press, whatever the page answers.
            view.drag.candidate = None;
            let (started, _) = self.fire_drag(id, source, "dragstart", x0, y0, "none", None);
            if !started {
                self.flush_drag_writes(id);
                return false;
            }
            let data = rustkit_bindings::MouseEventBindingData {
                client_x: viewport_x as f64,
                client_y: viewport_y as f64,
                screen_x: viewport_x as f64,
                screen_y: viewport_y as f64,
                button: -1,
                ..Default::default()
            };
            self.fire_mouse(id, source, "pointercancel", &data);
            if let Some(view) = self.views.get_mut(&id) {
                view.drag.active = Some(ActiveDrag {
                    source,
                    current: None,
                    operation: "none".into(),
                });
                // The release ends the drag: no `mouseup`, no `click`.
                view.press_target = None;
            }
        }
        // `:hover` follows the pointer; nothing else of the move is heard.
        self.pointer_moved(id, viewport_x, viewport_y, true, false);
        self.drag_iteration(id, viewport_x, viewport_y);
        true
    }

    /// One iteration of the processing model at VIEWPORT coordinates.
    fn drag_iteration(&mut self, id: EngineViewId, viewport_x: f32, viewport_y: f32) {
        let Some(drag) = self.views.get(&id).and_then(|v| v.drag.active.as_ref()) else {
            return;
        };
        let (source, current) = (drag.source, drag.current);
        let (go_on, _) = self.fire_drag(id, source, "drag", viewport_x, viewport_y, "none", None);
        if !go_on {
            // A cancelled `drag` is "no drag operation" for this turn.
            self.set_drag(id, |d| d.operation = "none".into());
            self.flush_drag_writes(id);
            return;
        }
        let under = self.element_at_point(id, viewport_x, viewport_y);
        if under != current {
            if let Some(entered) = under {
                self.fire_drag(
                    id,
                    entered,
                    "dragenter",
                    viewport_x,
                    viewport_y,
                    "none",
                    current,
                );
            }
            if let Some(left) = current {
                let op = self.drag_operation(id);
                self.fire_drag(id, left, "dragleave", viewport_x, viewport_y, &op, under);
            }
            self.set_drag(id, |d| d.current = under);
        }
        let operation = match under {
            Some(target) => {
                match self.fire_drag(id, target, "dragover", viewport_x, viewport_y, "none", None) {
                    (false, op) => op,
                    (true, _) => "none".to_string(),
                }
            }
            None => "none".to_string(),
        };
        self.set_drag(id, |d| d.operation = operation);
        self.flush_drag_writes(id);
    }

    /// A release: when a drag is in progress, end it there (`drop` or
    /// `dragleave` at the current target, then `dragend` at the source) and
    /// return what the release did; `None` when there was no drag.
    pub(crate) fn drag_release(
        &mut self,
        id: EngineViewId,
        viewport_x: f32,
        viewport_y: f32,
    ) -> Option<ClickOutcome> {
        let view = self.views.get_mut(&id)?;
        view.drag.candidate = None;
        view.drag.active.as_ref()?;
        if view.pointer_at != Some((viewport_x, viewport_y)) {
            self.pointer_moved(id, viewport_x, viewport_y, true, false);
            self.drag_iteration(id, viewport_x, viewport_y);
        }
        let view = self.views.get_mut(&id)?;
        let drag = view.drag.active.take()?;
        view.primary_button_down = false;
        view.press_target = None;
        view.press_settled_focus = false;
        view.compat_mouse_suppressed = false;
        let changed = std::mem::take(&mut *view.lay.active_chain.borrow_mut());
        view.pointer_restyle |= Self::chain_change_restyles(view, |reads| &reads.active, &changed);
        // The drag is over for the page's mouse events from here: a
        // listener's synchronous hit test sees no drag.
        let mut operation = drag.operation;
        if let Some(target) = drag.current {
            if operation != "none" {
                if let (false, op) =
                    self.fire_drag(id, target, "drop", viewport_x, viewport_y, &operation, None)
                {
                    operation = op;
                }
            } else {
                self.fire_drag(
                    id,
                    target,
                    "dragleave",
                    viewport_x,
                    viewport_y,
                    &operation,
                    None,
                );
            }
        } else {
            operation = "none".into();
        }
        self.fire_drag(
            id,
            drag.source,
            "dragend",
            viewport_x,
            viewport_y,
            &operation,
            None,
        );
        self.flush_drag_writes(id);
        self.settle_pointer_restyle(id);
        Some(ClickOutcome {
            focused: self.focused_tag(id),
            navigate: None,
        })
    }

    /// End the drag in progress without a drop: the current target hears
    /// `dragleave` and the source `dragend` with `dropEffect` `none`. For
    /// a release the engine will never see (the view lost focus, the host
    /// saw the button come up elsewhere), so the page does not stay in a
    /// drag. Does nothing when no drag is running; a pending candidate is
    /// dropped either way.
    pub fn cancel_drag(&mut self, id: EngineViewId) {
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        view.drag.candidate = None;
        let Some(drag) = view.drag.active.take() else {
            return;
        };
        let (x, y) = view.pointer_at.unwrap_or((0.0, 0.0));
        view.primary_button_down = false;
        view.press_target = None;
        view.press_settled_focus = false;
        view.compat_mouse_suppressed = false;
        let changed = std::mem::take(&mut *view.lay.active_chain.borrow_mut());
        view.pointer_restyle |= Self::chain_change_restyles(view, |reads| &reads.active, &changed);
        if let Some(current) = drag.current {
            self.fire_drag(id, current, "dragleave", x, y, "none", None);
        }
        self.fire_drag(id, drag.source, "dragend", x, y, "none", None);
        self.flush_drag_writes(id);
        self.settle_pointer_restyle(id);
    }

    /// The pointer left the view during a drag: the current target hears
    /// `dragleave` and there is none until it comes back.
    pub(crate) fn drag_left_view(&mut self, id: EngineViewId) {
        let Some((current, (x, y))) = self.views.get(&id).and_then(|v| {
            let drag = v.drag.active.as_ref()?;
            Some((drag.current?, v.pointer_at?))
        }) else {
            return;
        };
        self.fire_drag(id, current, "dragleave", x, y, "none", None);
        self.set_drag(id, |d| {
            d.current = None;
            d.operation = "none".into();
        });
        self.flush_drag_writes(id);
    }

    fn set_drag(&mut self, id: EngineViewId, f: impl FnOnce(&mut ActiveDrag)) {
        if let Some(drag) = self.views.get_mut(&id).and_then(|v| v.drag.active.as_mut()) {
            f(drag);
        }
    }

    fn drag_operation(&self, id: EngineViewId) -> String {
        self.views
            .get(&id)
            .and_then(|v| v.drag.active.as_ref())
            .map_or_else(|| "none".to_string(), |d| d.operation.clone())
    }

    fn flush_drag_writes(&mut self, id: EngineViewId) {
        if let Err(e) = self.flush_script_dom_writes(id, Some(RelayoutCause::Input)) {
            debug!(?id, error = %e, "relayout after a drag event failed");
        }
    }

    /// Fire one drag event at an element (raw NodeId) at VIEWPORT
    /// coordinates. Returns whether no listener cancelled it, and the
    /// operation the `dataTransfer` allows after its listeners.
    #[allow(clippy::too_many_arguments)]
    fn fire_drag(
        &mut self,
        id: EngineViewId,
        target: usize,
        event_type: &str,
        viewport_x: f32,
        viewport_y: f32,
        operation: &str,
        related_target: Option<usize>,
    ) -> (bool, String) {
        let Some(view) = self.views.get(&id) else {
            return (true, "none".into());
        };
        let origin = view
            .layout
            .as_ref()
            .and_then(|l| Self::box_origin(l, target))
            .unwrap_or((0.0, 0.0));
        let hit = Self::hit_at(view, viewport_x, viewport_y);
        let (doc_x, doc_y) = Self::layout_point(view, hit.as_ref(), viewport_x, viewport_y);
        let released = matches!(event_type, "drop" | "dragend");
        let data = rustkit_bindings::MouseEventBindingData {
            client_x: viewport_x as f64,
            client_y: viewport_y as f64,
            screen_x: viewport_x as f64,
            screen_y: viewport_y as f64,
            offset_x: (doc_x - origin.0) as f64,
            offset_y: (doc_y - origin.1) as f64,
            button: 0,
            buttons: if released { 0 } else { 1 },
            related_target,
            ..Default::default()
        };
        self.run_listeners(
            id,
            target,
            event_type,
            (true, "none".to_string()),
            |bindings| bindings.fire_drag_event(target, event_type, &data, operation),
        )
    }
}
