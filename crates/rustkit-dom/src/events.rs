//! # DOM Events
//!
//! DOM event types and dispatch mechanism implementing the DOM Events spec.
//! Supports capture and bubble phases, stopPropagation, and preventDefault.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Node, NodeId};

/// Unique identifier for an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventId(u64);

impl EventId {
    /// Create a new unique EventId.
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for EventId {
    fn default() -> Self {
        Self::new()
    }
}

/// Event phases as per the DOM spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EventPhase {
    /// No event is being processed.
    None = 0,
    /// Event is propagating through target's ancestors (capture).
    Capturing = 1,
    /// Event has arrived at the event target.
    AtTarget = 2,
    /// Event is propagating back up through ancestors (bubble).
    Bubbling = 3,
}

/// Common event interface for all DOM events.
#[derive(Debug, Clone)]
pub struct Event {
    /// Unique ID for this event.
    pub id: EventId,
    /// Event type (e.g., "click", "keydown").
    pub event_type: String,
    /// Whether the event bubbles.
    pub bubbles: bool,
    /// Whether the event is cancelable.
    pub cancelable: bool,
    /// Whether the event is composed (crosses shadow DOM boundary).
    pub composed: bool,
    /// Timestamp when the event was created.
    pub timestamp: u64,
    /// Current phase.
    phase: Cell<EventPhase>,
    /// The target node (where the event originated).
    target: RefCell<Option<NodeId>>,
    /// The current target (node currently handling the event).
    current_target: RefCell<Option<NodeId>>,
    /// Whether stopPropagation was called.
    propagation_stopped: Cell<bool>,
    /// Whether stopImmediatePropagation was called.
    immediate_propagation_stopped: Cell<bool>,
    /// Whether preventDefault was called.
    default_prevented: Cell<bool>,
    /// Whether the event is trusted (dispatched by the browser).
    pub is_trusted: bool,
}

impl Event {
    /// Create a new event.
    pub fn new(event_type: &str, bubbles: bool, cancelable: bool) -> Self {
        Self {
            id: EventId::new(),
            event_type: event_type.to_string(),
            bubbles,
            cancelable,
            composed: false,
            timestamp: Self::current_timestamp(),
            phase: Cell::new(EventPhase::None),
            target: RefCell::new(None),
            current_target: RefCell::new(None),
            propagation_stopped: Cell::new(false),
            immediate_propagation_stopped: Cell::new(false),
            default_prevented: Cell::new(false),
            is_trusted: false,
        }
    }

    /// Create a trusted event (from the browser).
    pub fn new_trusted(event_type: &str, bubbles: bool, cancelable: bool) -> Self {
        let mut event = Self::new(event_type, bubbles, cancelable);
        event.is_trusted = true;
        event
    }

    /// Get the current phase.
    pub fn phase(&self) -> EventPhase {
        self.phase.get()
    }

    /// Get the target node ID.
    pub fn target(&self) -> Option<NodeId> {
        *self.target.borrow()
    }

    /// Get the current target node ID.
    pub fn current_target(&self) -> Option<NodeId> {
        *self.current_target.borrow()
    }

    /// Stop propagation of the event.
    pub fn stop_propagation(&self) {
        self.propagation_stopped.set(true);
    }

    /// Stop immediate propagation of the event.
    pub fn stop_immediate_propagation(&self) {
        self.propagation_stopped.set(true);
        self.immediate_propagation_stopped.set(true);
    }

    /// Prevent the default action.
    pub fn prevent_default(&self) {
        if self.cancelable {
            self.default_prevented.set(true);
        }
    }

    /// Check if propagation is stopped.
    pub fn propagation_stopped(&self) -> bool {
        self.propagation_stopped.get()
    }

    /// Check if immediate propagation is stopped.
    pub fn immediate_propagation_stopped(&self) -> bool {
        self.immediate_propagation_stopped.get()
    }

    /// Check if the default action was prevented.
    pub fn default_prevented(&self) -> bool {
        self.default_prevented.get()
    }

    /// Set the phase (internal use).
    pub(crate) fn set_phase(&self, phase: EventPhase) {
        self.phase.set(phase);
    }

    /// Set the target (internal use).
    pub(crate) fn set_target(&self, target: NodeId) {
        *self.target.borrow_mut() = Some(target);
    }

    /// Set the current target (internal use).
    pub(crate) fn set_current_target(&self, target: Option<NodeId>) {
        *self.current_target.borrow_mut() = target;
    }

    fn current_timestamp() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Mouse event data.
#[derive(Debug, Clone)]
pub struct MouseEventData {
    /// X coordinate relative to the viewport.
    pub client_x: f64,
    /// Y coordinate relative to the viewport.
    pub client_y: f64,
    /// X coordinate relative to the screen.
    pub screen_x: f64,
    /// Y coordinate relative to the screen.
    pub screen_y: f64,
    /// X coordinate relative to the target element.
    pub offset_x: f64,
    /// Y coordinate relative to the target element.
    pub offset_y: f64,
    /// Which mouse button triggered the event.
    pub button: i16,
    /// Currently pressed buttons bitmask.
    pub buttons: u16,
    /// Whether Ctrl key was pressed.
    pub ctrl_key: bool,
    /// Whether Alt key was pressed.
    pub alt_key: bool,
    /// Whether Shift key was pressed.
    pub shift_key: bool,
    /// Whether Meta (Windows/Command) key was pressed.
    pub meta_key: bool,
    /// Related target (for enter/leave events).
    pub related_target: Option<NodeId>,
}

impl Default for MouseEventData {
    fn default() -> Self {
        Self {
            client_x: 0.0,
            client_y: 0.0,
            screen_x: 0.0,
            screen_y: 0.0,
            offset_x: 0.0,
            offset_y: 0.0,
            button: 0,
            buttons: 0,
            ctrl_key: false,
            alt_key: false,
            shift_key: false,
            meta_key: false,
            related_target: None,
        }
    }
}

/// Keyboard event data.
#[derive(Debug, Clone, Default)]
pub struct KeyboardEventData {
    /// The key value.
    pub key: String,
    /// The key code.
    pub code: String,
    /// Whether this is a repeat event.
    pub repeat: bool,
    /// Whether Ctrl key was pressed.
    pub ctrl_key: bool,
    /// Whether Alt key was pressed.
    pub alt_key: bool,
    /// Whether Shift key was pressed.
    pub shift_key: bool,
    /// Whether Meta key was pressed.
    pub meta_key: bool,
    /// The location of the key.
    pub location: u32,
}


/// Focus event data.
#[derive(Debug, Clone, Default)]
pub struct FocusEventData {
    /// The related target (element losing/gaining focus).
    pub related_target: Option<NodeId>,
}


/// Input event data.
#[derive(Debug, Clone)]
pub struct InputEventData {
    /// The input data (characters entered).
    pub data: Option<String>,
    /// The input type.
    pub input_type: String,
    /// Whether the input event is composed.
    pub is_composing: bool,
}

impl Default for InputEventData {
    fn default() -> Self {
        Self {
            data: None,
            input_type: "insertText".to_string(),
            is_composing: false,
        }
    }
}

/// DOM event with type-specific data.
#[derive(Debug, Clone)]
pub enum DomEvent {
    /// Generic event.
    Generic(Event),
    /// Mouse event.
    Mouse(Event, MouseEventData),
    /// Keyboard event.
    Keyboard(Event, KeyboardEventData),
    /// Focus event.
    Focus(Event, FocusEventData),
    /// Input event.
    Input(Event, InputEventData),
}

impl DomEvent {
    /// Get the base event.
    pub fn event(&self) -> &Event {
        match self {
            DomEvent::Generic(e) => e,
            DomEvent::Mouse(e, _) => e,
            DomEvent::Keyboard(e, _) => e,
            DomEvent::Focus(e, _) => e,
            DomEvent::Input(e, _) => e,
        }
    }

    /// Get mutable access to the base event.
    pub fn event_mut(&mut self) -> &mut Event {
        match self {
            DomEvent::Generic(e) => e,
            DomEvent::Mouse(e, _) => e,
            DomEvent::Keyboard(e, _) => e,
            DomEvent::Focus(e, _) => e,
            DomEvent::Input(e, _) => e,
        }
    }

    /// Create a mouse event.
    pub fn mouse(event_type: &str, bubbles: bool, data: MouseEventData) -> Self {
        let event = Event::new_trusted(event_type, bubbles, true);
        DomEvent::Mouse(event, data)
    }

    /// Create a keyboard event.
    pub fn keyboard(event_type: &str, data: KeyboardEventData) -> Self {
        let event = Event::new_trusted(event_type, true, true);
        DomEvent::Keyboard(event, data)
    }

    /// Create a focus event.
    pub fn focus(event_type: &str, data: FocusEventData) -> Self {
        // focus/blur don't bubble, focusin/focusout do
        let bubbles = event_type == "focusin" || event_type == "focusout";
        let event = Event::new_trusted(event_type, bubbles, false);
        DomEvent::Focus(event, data)
    }

    /// Create an input event.
    pub fn input(data: InputEventData) -> Self {
        let event = Event::new_trusted("input", true, false);
        DomEvent::Input(event, data)
    }
}

/// Options for adding an event listener.
#[derive(Debug, Clone, Default)]
pub struct AddEventListenerOptions {
    /// If true, the listener is invoked during capture phase.
    pub capture: bool,
    /// If true, the listener is automatically removed after first invocation.
    pub once: bool,
    /// If true, indicates that the listener will never call preventDefault.
    pub passive: bool,
}

/// An event listener callback.
pub type EventListenerCallback = Box<dyn Fn(&DomEvent) + 'static>;

/// A registered event listener.
struct EventListener {
    callback: EventListenerCallback,
    options: AddEventListenerOptions,
}

/// Event target mixin - manages event listeners for a node.
#[derive(Default)]
pub struct EventTarget {
    /// Listeners keyed by event type.
    listeners: RefCell<HashMap<String, Vec<EventListener>>>,
}

impl EventTarget {
    /// Create a new event target.
    pub fn new() -> Self {
        Self::default()
    }

    /// Move `other`'s listeners onto this target, leaving `other` with none.
    pub(crate) fn adopt_listeners(&self, other: &EventTarget) {
        self.listeners.swap(&other.listeners);
    }

    /// Add an event listener.
    pub fn add_event_listener(
        &self,
        event_type: &str,
        callback: EventListenerCallback,
        options: AddEventListenerOptions,
    ) {
        let mut listeners = self.listeners.borrow_mut();
        let list = listeners.entry(event_type.to_string()).or_default();
        list.push(EventListener { callback, options });
    }

    /// Remove an event listener.
    /// Note: This is simplified - in a full implementation, we'd need to identify listeners somehow.
    pub fn remove_all_listeners(&self, event_type: &str) {
        let mut listeners = self.listeners.borrow_mut();
        listeners.remove(event_type);
    }

    /// Check if there are any listeners for an event type.
    pub fn has_listeners(&self, event_type: &str) -> bool {
        let listeners = self.listeners.borrow();
        listeners
            .get(event_type)
            .map(|l| !l.is_empty())
            .unwrap_or(false)
    }

    /// Invoke listeners for an event.
    /// Returns indices of listeners to remove (for `once` listeners).
    pub fn invoke_listeners(&self, event: &DomEvent, phase: EventPhase) -> Vec<usize> {
        let listeners = self.listeners.borrow();
        let event_type = &event.event().event_type;

        let mut to_remove = Vec::new();

        if let Some(list) = listeners.get(event_type) {
            for (i, listener) in list.iter().enumerate() {
                // Check if listener should fire in this phase
                let should_fire = match phase {
                    EventPhase::Capturing => listener.options.capture,
                    EventPhase::AtTarget => true,
                    EventPhase::Bubbling => !listener.options.capture,
                    EventPhase::None => false,
                };

                if should_fire {
                    (listener.callback)(event);

                    if listener.options.once {
                        to_remove.push(i);
                    }

                    if event.event().immediate_propagation_stopped() {
                        break;
                    }
                }
            }
        }

        to_remove
    }

    /// Remove listeners at the given indices.
    pub fn remove_listeners(&self, event_type: &str, indices: Vec<usize>) {
        if indices.is_empty() {
            return;
        }

        let mut listeners = self.listeners.borrow_mut();
        if let Some(list) = listeners.get_mut(event_type) {
            // Remove in reverse order to preserve indices
            for i in indices.into_iter().rev() {
                if i < list.len() {
                    list.remove(i);
                }
            }
        }
    }
}

impl std::fmt::Debug for EventTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventTarget")
            .field("listener_count", &self.listeners.borrow().len())
            .finish()
    }
}

/// Event dispatcher for propagating events through the DOM tree.
pub struct EventDispatcher;

impl EventDispatcher {
    /// Dispatch an event to a target node.
    /// Returns true if the event was not prevented.
    pub fn dispatch(event: &mut DomEvent, target: &Rc<Node>, ancestors: &[Rc<Node>]) -> bool {
        // Get event info we need upfront
        let event_type = event.event().event_type.clone();
        let bubbles = event.event().bubbles;

        // Set target
        event.event().set_target(target.id);

        // Build the propagation path (ancestors + target)
        // Ancestors should be ordered from root to parent
        let mut path: Vec<&Rc<Node>> = ancestors.iter().collect();
        path.push(target);

        // Capture phase (root to target, excluding target)
        event.event().set_phase(EventPhase::Capturing);
        for node in &path[..path.len() - 1] {
            if event.event().propagation_stopped() {
                break;
            }
            event.event().set_current_target(Some(node.id));
            let to_remove = node
                .event_target
                .invoke_listeners(event, EventPhase::Capturing);
            node.event_target.remove_listeners(&event_type, to_remove);
        }

        // At target phase
        if !event.event().propagation_stopped() {
            event.event().set_phase(EventPhase::AtTarget);
            event.event().set_current_target(Some(target.id));
            let to_remove = target
                .event_target
                .invoke_listeners(event, EventPhase::AtTarget);
            target.event_target.remove_listeners(&event_type, to_remove);
        }

        // Bubble phase (target to root, excluding target)
        if bubbles && !event.event().propagation_stopped() {
            event.event().set_phase(EventPhase::Bubbling);
            for node in path[..path.len() - 1].iter().rev() {
                if event.event().propagation_stopped() {
                    break;
                }
                event.event().set_current_target(Some(node.id));
                let to_remove = node
                    .event_target
                    .invoke_listeners(event, EventPhase::Bubbling);
                node.event_target.remove_listeners(&event_type, to_remove);
            }
        }

        // Reset state
        event.event().set_phase(EventPhase::None);
        event.event().set_current_target(None);

        !event.event().default_prevented()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn test_event_creation() {
        let event = Event::new("click", true, true);
        assert_eq!(event.event_type, "click");
        assert!(event.bubbles);
        assert!(event.cancelable);
        assert!(!event.is_trusted);
    }

    #[test]
    fn test_trusted_event() {
        let event = Event::new_trusted("keydown", true, true);
        assert!(event.is_trusted);
    }

    #[test]
    fn test_stop_propagation() {
        let event = Event::new("click", true, true);
        assert!(!event.propagation_stopped());

        event.stop_propagation();
        assert!(event.propagation_stopped());
        assert!(!event.immediate_propagation_stopped());
    }

    #[test]
    fn test_stop_immediate_propagation() {
        let event = Event::new("click", true, true);
        event.stop_immediate_propagation();
        assert!(event.propagation_stopped());
        assert!(event.immediate_propagation_stopped());
    }

    #[test]
    fn test_prevent_default() {
        let cancelable = Event::new("click", true, true);
        cancelable.prevent_default();
        assert!(cancelable.default_prevented());

        let not_cancelable = Event::new("load", false, false);
        not_cancelable.prevent_default();
        assert!(!not_cancelable.default_prevented());
    }

    #[test]
    fn test_event_target_add_listener() {
        let target = EventTarget::new();

        let called = Rc::new(Cell::new(false));
        let called_clone = called.clone();

        target.add_event_listener(
            "click",
            Box::new(move |_| called_clone.set(true)),
            AddEventListenerOptions::default(),
        );

        assert!(target.has_listeners("click"));
        assert!(!target.has_listeners("keydown"));
    }

    #[test]
    fn test_dom_event_types() {
        let mouse = DomEvent::mouse("click", true, MouseEventData::default());
        assert_eq!(mouse.event().event_type, "click");

        let keyboard = DomEvent::keyboard("keydown", KeyboardEventData::default());
        assert_eq!(keyboard.event().event_type, "keydown");

        let focus = DomEvent::focus("focus", FocusEventData::default());
        assert!(!focus.event().bubbles); // focus doesn't bubble

        let focusin = DomEvent::focus("focusin", FocusEventData::default());
        assert!(focusin.event().bubbles); // focusin does bubble
    }
}
