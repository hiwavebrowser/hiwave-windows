//! # RustKit Events
//!
//! Complete event handling system for the RustKit browser engine.
//!
//! Implements:
//! - requestAnimationFrame (RAF) callback system
//! - Hover state tracking (mouseenter/mouseleave)
//! - Transition and animation events
//! - Pointer Events API (modern unified input)
//! - Touch Events (multi-touch support)
//! - Drag Events (DnD API)
//! - Full event capture/bubble phases
//! - Focus management (tab order, :focus-visible)
//! - MessageEvent (postMessage)

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rustkit_dom::NodeId;
use tracing::{debug, trace};

// ==================== Event Phase ====================

/// Event propagation phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPhase {
    /// No event is being processed.
    None = 0,
    /// Capture phase - event travels from Window to target.
    Capturing = 1,
    /// At target - event is at the target element.
    AtTarget = 2,
    /// Bubble phase - event travels from target to Window.
    Bubbling = 3,
}

// ==================== Base Event ====================

/// Base event interface matching DOM Event spec.
#[derive(Debug, Clone)]
pub struct Event {
    /// Event type (e.g., "click", "keydown").
    pub event_type: String,
    /// Whether the event bubbles up through the DOM.
    pub bubbles: bool,
    /// Whether the event can be cancelled.
    pub cancelable: bool,
    /// Target element.
    pub target: Option<NodeId>,
    /// Current target during propagation.
    pub current_target: Option<NodeId>,
    /// Current phase of event flow.
    pub event_phase: EventPhase,
    /// High-resolution timestamp.
    pub time_stamp: f64,
    /// Whether default action was prevented.
    pub default_prevented: bool,
    /// Whether propagation was stopped.
    pub propagation_stopped: bool,
    /// Whether immediate propagation was stopped.
    pub immediate_propagation_stopped: bool,
    /// Whether the event is trusted (user-generated).
    pub is_trusted: bool,
    /// Whether the event is composed (crosses shadow DOM).
    pub composed: bool,
}

impl Event {
    /// Create a new event.
    pub fn new(event_type: &str, bubbles: bool, cancelable: bool) -> Self {
        Self {
            event_type: event_type.to_string(),
            bubbles,
            cancelable,
            target: None,
            current_target: None,
            event_phase: EventPhase::None,
            time_stamp: Instant::now().elapsed().as_secs_f64() * 1000.0,
            default_prevented: false,
            propagation_stopped: false,
            immediate_propagation_stopped: false,
            is_trusted: false,
            composed: false,
        }
    }

    /// Prevent the default action.
    pub fn prevent_default(&mut self) {
        if self.cancelable {
            self.default_prevented = true;
        }
    }

    /// Stop event propagation.
    pub fn stop_propagation(&mut self) {
        self.propagation_stopped = true;
    }

    /// Stop immediate propagation.
    pub fn stop_immediate_propagation(&mut self) {
        self.propagation_stopped = true;
        self.immediate_propagation_stopped = true;
    }
}

// ==================== Pointer Events ====================

/// Pointer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerType {
    #[default]
    Mouse,
    Pen,
    Touch,
    Unknown,
}

impl std::fmt::Display for PointerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PointerType::Mouse => write!(f, "mouse"),
            PointerType::Pen => write!(f, "pen"),
            PointerType::Touch => write!(f, "touch"),
            PointerType::Unknown => write!(f, ""),
        }
    }
}

/// Pointer event data (modern unified input API).
#[derive(Debug, Clone, Default)]
pub struct PointerEventData {
    // From MouseEvent
    pub client_x: f64,
    pub client_y: f64,
    pub screen_x: f64,
    pub screen_y: f64,
    pub page_x: f64,
    pub page_y: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    pub movement_x: f64,
    pub movement_y: f64,
    pub button: i16,
    pub buttons: u16,
    pub ctrl_key: bool,
    pub alt_key: bool,
    pub shift_key: bool,
    pub meta_key: bool,

    // Pointer-specific
    /// Unique pointer identifier.
    pub pointer_id: i32,
    /// Width of the contact geometry.
    pub width: f64,
    /// Height of the contact geometry.
    pub height: f64,
    /// Normalized pressure (0.0 - 1.0).
    pub pressure: f32,
    /// Tangential pressure (pen barrel button).
    pub tangential_pressure: f32,
    /// Tilt angle along X axis (-90 to 90).
    pub tilt_x: i32,
    /// Tilt angle along Y axis (-90 to 90).
    pub tilt_y: i32,
    /// Rotation of the pointer (0 to 359).
    pub twist: i32,
    /// Pointer type.
    pub pointer_type: PointerType,
    /// Whether this is the primary pointer.
    pub is_primary: bool,
}

impl PointerEventData {
    /// Create a PointerEvent from mouse input.
    pub fn from_mouse(
        client_x: f64,
        client_y: f64,
        button: i16,
        buttons: u16,
        modifiers: (bool, bool, bool, bool),
    ) -> Self {
        Self {
            client_x,
            client_y,
            screen_x: client_x,
            screen_y: client_y,
            page_x: client_x,
            page_y: client_y,
            button,
            buttons,
            ctrl_key: modifiers.0,
            alt_key: modifiers.1,
            shift_key: modifiers.2,
            meta_key: modifiers.3,
            pointer_id: 1, // Mouse is always pointer 1
            width: 1.0,
            height: 1.0,
            pressure: if buttons > 0 { 0.5 } else { 0.0 },
            pointer_type: PointerType::Mouse,
            is_primary: true,
            ..Default::default()
        }
    }

    /// Create a PointerEvent from touch input.
    pub fn from_touch(touch: &Touch, is_primary: bool) -> Self {
        Self {
            client_x: touch.client_x,
            client_y: touch.client_y,
            screen_x: touch.screen_x,
            screen_y: touch.screen_y,
            page_x: touch.page_x,
            page_y: touch.page_y,
            pointer_id: touch.identifier as i32,
            width: touch.radius_x * 2.0,
            height: touch.radius_y * 2.0,
            pressure: touch.force,
            pointer_type: PointerType::Touch,
            is_primary,
            button: 0,
            buttons: 1,
            ..Default::default()
        }
    }
}

// ==================== Touch Events ====================

/// A single touch point.
#[derive(Debug, Clone, Default)]
pub struct Touch {
    /// Unique identifier for the touch.
    pub identifier: u64,
    /// Target element.
    pub target: Option<NodeId>,
    /// X coordinate relative to viewport.
    pub client_x: f64,
    /// Y coordinate relative to viewport.
    pub client_y: f64,
    /// X coordinate relative to screen.
    pub screen_x: f64,
    /// Y coordinate relative to screen.
    pub screen_y: f64,
    /// X coordinate relative to page.
    pub page_x: f64,
    /// Y coordinate relative to page.
    pub page_y: f64,
    /// Radius of the touch area (X).
    pub radius_x: f64,
    /// Radius of the touch area (Y).
    pub radius_y: f64,
    /// Rotation angle of the touch area.
    pub rotation_angle: f64,
    /// Force of the touch (0.0 - 1.0).
    pub force: f32,
}

impl Touch {
    /// Generate a new unique touch identifier.
    pub fn new_id() -> u64 {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }
}

/// Touch event data with list of active touches.
#[derive(Debug, Clone, Default)]
pub struct TouchEventData {
    /// List of all touches currently on the surface.
    pub touches: Vec<Touch>,
    /// Touches that have changed since last event.
    pub changed_touches: Vec<Touch>,
    /// Touches that started on the target element.
    pub target_touches: Vec<Touch>,
    /// Modifier keys.
    pub ctrl_key: bool,
    pub alt_key: bool,
    pub shift_key: bool,
    pub meta_key: bool,
}

// ==================== Drag Events ====================

/// Data transfer object for drag-and-drop.
#[derive(Debug, Clone, Default)]
pub struct DataTransfer {
    /// Drop effect: "none", "copy", "move", "link".
    pub drop_effect: String,
    /// Allowed effects: "none", "copy", "move", "link", "copyMove", etc.
    pub effect_allowed: String,
    /// Files being dragged.
    pub files: Vec<DroppedFile>,
    /// Data items by type.
    pub items: HashMap<String, String>,
    /// Types of data available.
    pub types: Vec<String>,
}

impl DataTransfer {
    /// Create a new DataTransfer.
    pub fn new() -> Self {
        Self {
            drop_effect: "none".to_string(),
            effect_allowed: "uninitialized".to_string(),
            ..Default::default()
        }
    }

    /// Set data for a type.
    pub fn set_data(&mut self, format: &str, data: &str) {
        if !self.types.contains(&format.to_string()) {
            self.types.push(format.to_string());
        }
        self.items.insert(format.to_string(), data.to_string());
    }

    /// Get data for a type.
    pub fn get_data(&self, format: &str) -> Option<&String> {
        self.items.get(format)
    }

    /// Clear data.
    pub fn clear_data(&mut self, format: Option<&str>) {
        if let Some(fmt) = format {
            self.items.remove(fmt);
            self.types.retain(|t| t != fmt);
        } else {
            self.items.clear();
            self.types.clear();
        }
    }
}

/// A dropped file.
#[derive(Debug, Clone)]
pub struct DroppedFile {
    pub name: String,
    pub size: u64,
    pub mime_type: String,
    pub path: Option<std::path::PathBuf>,
}

/// Drag event data.
#[derive(Debug, Clone)]
pub struct DragEventData {
    /// Mouse position.
    pub client_x: f64,
    pub client_y: f64,
    pub screen_x: f64,
    pub screen_y: f64,
    /// Modifier keys.
    pub ctrl_key: bool,
    pub alt_key: bool,
    pub shift_key: bool,
    pub meta_key: bool,
    /// Data being transferred.
    pub data_transfer: DataTransfer,
}

impl Default for DragEventData {
    fn default() -> Self {
        Self {
            client_x: 0.0,
            client_y: 0.0,
            screen_x: 0.0,
            screen_y: 0.0,
            ctrl_key: false,
            alt_key: false,
            shift_key: false,
            meta_key: false,
            data_transfer: DataTransfer::new(),
        }
    }
}

// ==================== Transition/Animation Events ====================

/// Transition event data.
#[derive(Debug, Clone)]
pub struct TransitionEventData {
    /// Name of the CSS property associated with the transition.
    pub property_name: String,
    /// Duration of the transition in seconds.
    pub elapsed_time: f64,
    /// Pseudo-element on which the transition runs (or empty).
    pub pseudo_element: String,
}

/// Animation event data.
#[derive(Debug, Clone)]
pub struct AnimationEventData {
    /// Name of the CSS animation.
    pub animation_name: String,
    /// Time the animation has been running in seconds.
    pub elapsed_time: f64,
    /// Pseudo-element on which the animation runs (or empty).
    pub pseudo_element: String,
}

// ==================== Message Event ====================

/// Message event data for postMessage.
#[derive(Debug, Clone)]
pub struct MessageEventData {
    /// The data sent by the message emitter.
    pub data: String,
    /// The origin of the message sender.
    pub origin: String,
    /// Last event ID (for server-sent events).
    pub last_event_id: String,
    /// Source window (represented as ID).
    pub source: Option<u64>,
    /// Message ports.
    pub ports: Vec<u64>,
}

impl Default for MessageEventData {
    fn default() -> Self {
        Self {
            data: String::new(),
            origin: String::new(),
            last_event_id: String::new(),
            source: None,
            ports: Vec::new(),
        }
    }
}

// ==================== requestAnimationFrame ====================

/// Unique callback ID for RAF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RafCallbackId(u64);

impl RafCallbackId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// A pending RAF callback.
#[derive(Debug)]
pub struct RafCallback {
    pub id: RafCallbackId,
    pub callback: String, // JS code
    pub cancelled: bool,
}

/// RAF scheduler manages requestAnimationFrame callbacks.
#[derive(Debug, Default)]
pub struct RafScheduler {
    /// Pending callbacks for the next frame.
    pending: VecDeque<RafCallback>,
    /// Callbacks being executed (double-buffered).
    executing: Vec<RafCallback>,
    /// Cancelled callback IDs.
    cancelled: HashSet<RafCallbackId>,
    /// Last frame time.
    last_frame_time: Option<Instant>,
    /// Target frame duration (16.67ms for 60fps).
    target_frame_duration: Duration,
}

impl RafScheduler {
    /// Create a new RAF scheduler.
    pub fn new() -> Self {
        Self {
            target_frame_duration: Duration::from_secs_f64(1.0 / 60.0),
            ..Default::default()
        }
    }

    /// Request animation frame callback.
    pub fn request(&mut self, callback: String) -> RafCallbackId {
        let id = RafCallbackId::new();
        self.pending.push_back(RafCallback {
            id,
            callback,
            cancelled: false,
        });
        trace!("RAF requested: {:?}", id);
        id
    }

    /// Cancel a pending callback.
    pub fn cancel(&mut self, id: RafCallbackId) {
        self.cancelled.insert(id);
        trace!("RAF cancelled: {:?}", id);
    }

    /// Check if there are pending callbacks.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Process callbacks for the current frame.
    /// Returns the callbacks to execute with their timestamps.
    pub fn tick(&mut self) -> Vec<(RafCallbackId, String, f64)> {
        let now = Instant::now();
        let timestamp = if let Some(last) = self.last_frame_time {
            last.elapsed().as_secs_f64() * 1000.0
        } else {
            0.0
        };
        self.last_frame_time = Some(now);

        // Move pending to executing, filtering cancelled
        self.executing.clear();
        while let Some(cb) = self.pending.pop_front() {
            if !self.cancelled.contains(&cb.id) {
                self.executing.push(cb);
            }
        }

        // Clear cancelled set
        self.cancelled.clear();

        // Return callbacks with their timestamps
        self.executing
            .iter()
            .map(|cb| (cb.id, cb.callback.clone(), timestamp))
            .collect()
    }

    /// Get time until next frame should run.
    pub fn time_to_next_frame(&self) -> Duration {
        if let Some(last) = self.last_frame_time {
            let elapsed = last.elapsed();
            if elapsed < self.target_frame_duration {
                self.target_frame_duration - elapsed
            } else {
                Duration::ZERO
            }
        } else {
            Duration::ZERO
        }
    }
}

// ==================== Hover State Tracking ====================

/// Tracks hover state for mouseenter/mouseleave events.
#[derive(Debug, Default)]
pub struct HoverTracker {
    /// Currently hovered elements (from root to deepest).
    hovered_path: Vec<NodeId>,
    /// Elements that have :hover pseudo-class.
    hover_set: HashSet<NodeId>,
}

impl HoverTracker {
    /// Create a new hover tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Update hover state with a new target.
    /// Returns (elements_entered, elements_left).
    pub fn update(&mut self, new_path: Vec<NodeId>) -> (Vec<NodeId>, Vec<NodeId>) {
        let old_set: HashSet<_> = self.hovered_path.iter().cloned().collect();
        let new_set: HashSet<_> = new_path.iter().cloned().collect();

        // Elements that were hovered but no longer are
        let left: Vec<_> = self.hovered_path
            .iter()
            .filter(|n| !new_set.contains(n))
            .cloned()
            .collect();

        // Elements that are now hovered but weren't before
        let entered: Vec<_> = new_path
            .iter()
            .filter(|n| !old_set.contains(n))
            .cloned()
            .collect();

        self.hovered_path = new_path;
        self.hover_set = new_set;

        debug!(
            "Hover update: {} entered, {} left",
            entered.len(),
            left.len()
        );

        (entered, left)
    }

    /// Check if an element is hovered.
    pub fn is_hovered(&self, node_id: NodeId) -> bool {
        self.hover_set.contains(&node_id)
    }

    /// Get the current hover path.
    pub fn hover_path(&self) -> &[NodeId] {
        &self.hovered_path
    }

    /// Clear all hover state.
    pub fn clear(&mut self) {
        self.hovered_path.clear();
        self.hover_set.clear();
    }
}

// ==================== Focus Management ====================

/// Focus state for :focus-visible pseudo-class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusVisibility {
    /// Focus was triggered by mouse - hide focus ring.
    #[default]
    Mouse,
    /// Focus was triggered by keyboard - show focus ring.
    Keyboard,
    /// Focus ring should always be shown.
    Always,
}

/// Focusable element info.
#[derive(Debug, Clone)]
pub struct FocusableElement {
    pub node_id: NodeId,
    pub tab_index: i32,
    pub is_disabled: bool,
    pub is_inert: bool,
}

/// Focus manager handles tab order and focus state.
#[derive(Debug, Default)]
pub struct FocusManager {
    /// Currently focused element.
    active_element: Option<NodeId>,
    /// Focus visibility state.
    focus_visibility: FocusVisibility,
    /// Tab-ordered focusable elements.
    tab_order: Vec<FocusableElement>,
    /// Whether we're in a focus trap (modal).
    focus_trap: Option<NodeId>,
    /// Last input method (for focus-visible detection).
    last_input_was_keyboard: bool,
}

impl FocusManager {
    /// Create a new focus manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the currently focused element.
    pub fn active_element(&self) -> Option<NodeId> {
        self.active_element
    }

    /// Check if focus ring should be visible.
    pub fn is_focus_visible(&self) -> bool {
        matches!(self.focus_visibility, FocusVisibility::Keyboard | FocusVisibility::Always)
    }

    /// Set the focused element.
    /// Returns (old_focus, new_focus) for dispatching events.
    pub fn set_focus(&mut self, node_id: Option<NodeId>, from_keyboard: bool) -> (Option<NodeId>, Option<NodeId>) {
        let old = self.active_element;
        self.active_element = node_id;
        self.last_input_was_keyboard = from_keyboard;
        self.focus_visibility = if from_keyboard {
            FocusVisibility::Keyboard
        } else {
            FocusVisibility::Mouse
        };

        debug!(
            "Focus changed: {:?} -> {:?} (keyboard: {})",
            old, node_id, from_keyboard
        );

        (old, node_id)
    }

    /// Update the tab order from a list of focusable elements.
    pub fn update_tab_order(&mut self, elements: Vec<FocusableElement>) {
        // Sort by tabindex:
        // 1. tabindex > 0 in ascending order
        // 2. tabindex = 0 in DOM order
        // 3. tabindex < 0 are not tabbable (but still focusable via JS)
        
        let mut positive: Vec<_> = elements.iter()
            .filter(|e| e.tab_index > 0 && !e.is_disabled && !e.is_inert)
            .cloned()
            .collect();
        positive.sort_by_key(|e| e.tab_index);

        let zero: Vec<_> = elements.iter()
            .filter(|e| e.tab_index == 0 && !e.is_disabled && !e.is_inert)
            .cloned()
            .collect();

        self.tab_order = positive;
        self.tab_order.extend(zero);

        trace!("Tab order updated: {} elements", self.tab_order.len());
    }

    /// Move focus to the next element in tab order.
    /// Returns the new focus target.
    pub fn move_next(&mut self) -> Option<NodeId> {
        if self.tab_order.is_empty() {
            return None;
        }

        let current_idx = self.active_element
            .and_then(|n| self.tab_order.iter().position(|e| e.node_id == n));

        let next_idx = match current_idx {
            Some(idx) => (idx + 1) % self.tab_order.len(),
            None => 0,
        };

        // Handle focus trap
        if let Some(_trap) = self.focus_trap {
            // Only move within trap - for now just return current
            // TODO: proper trap handling
            return self.active_element;
        }

        self.tab_order.get(next_idx).map(|e| e.node_id)
    }

    /// Move focus to the previous element in tab order.
    /// Returns the new focus target.
    pub fn move_prev(&mut self) -> Option<NodeId> {
        if self.tab_order.is_empty() {
            return None;
        }

        let current_idx = self.active_element
            .and_then(|n| self.tab_order.iter().position(|e| e.node_id == n));

        let prev_idx = match current_idx {
            Some(idx) if idx > 0 => idx - 1,
            Some(_) => self.tab_order.len() - 1,
            None => self.tab_order.len() - 1,
        };

        self.tab_order.get(prev_idx).map(|e| e.node_id)
    }

    /// Set a focus trap (for modals/dialogs).
    pub fn set_focus_trap(&mut self, container: Option<NodeId>) {
        self.focus_trap = container;
        debug!("Focus trap set: {:?}", container);
    }

    /// Check if focus is trapped.
    pub fn is_focus_trapped(&self) -> bool {
        self.focus_trap.is_some()
    }

    /// Record that keyboard was used (for focus-visible).
    pub fn record_keyboard_input(&mut self) {
        self.last_input_was_keyboard = true;
    }

    /// Record that mouse was used (for focus-visible).
    pub fn record_mouse_input(&mut self) {
        self.last_input_was_keyboard = false;
    }
}

// ==================== Event Dispatch ====================

/// Event listener options.
#[derive(Debug, Clone, Default)]
pub struct EventListenerOptions {
    /// Whether to use capture phase.
    pub capture: bool,
    /// Whether listener should only fire once.
    pub once: bool,
    /// Whether listener is passive (can't call preventDefault).
    pub passive: bool,
    /// Abort signal to remove listener.
    pub signal: Option<u64>,
}

/// An event listener with options.
#[derive(Debug)]
pub struct EventListenerEntry {
    pub id: u64,
    pub event_type: String,
    pub callback: String,
    pub options: EventListenerOptions,
    pub removed: bool,
}

/// Event dispatcher manages event flow through the DOM.
#[derive(Debug, Default)]
pub struct EventDispatcher {
    /// Listeners by node ID.
    listeners: HashMap<NodeId, Vec<EventListenerEntry>>,
    /// Next listener ID.
    next_id: u64,
}

impl EventDispatcher {
    /// Create a new event dispatcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an event listener.
    pub fn add_listener(
        &mut self,
        node_id: NodeId,
        event_type: &str,
        callback: &str,
        options: EventListenerOptions,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let entry = EventListenerEntry {
            id,
            event_type: event_type.to_string(),
            callback: callback.to_string(),
            options,
            removed: false,
        };

        self.listeners
            .entry(node_id)
            .or_default()
            .push(entry);

        trace!("Added listener {} for {} on {:?}", id, event_type, node_id);
        id
    }

    /// Remove an event listener by ID.
    pub fn remove_listener(&mut self, node_id: NodeId, listener_id: u64) -> bool {
        if let Some(listeners) = self.listeners.get_mut(&node_id) {
            if let Some(entry) = listeners.iter_mut().find(|e| e.id == listener_id) {
                entry.removed = true;
                return true;
            }
        }
        false
    }

    /// Remove all listeners for a node.
    pub fn remove_all_listeners(&mut self, node_id: NodeId) {
        self.listeners.remove(&node_id);
    }

    /// Build the propagation path for an event.
    pub fn build_propagation_path(&self, target: NodeId, ancestors: &[NodeId]) -> Vec<NodeId> {
        // Path is: [Window, ..., grandparent, parent, target]
        let mut path: Vec<_> = ancestors.iter().rev().cloned().collect();
        path.push(target);
        path
    }

    /// Get listeners for a node and event type in the correct phase.
    pub fn get_listeners(
        &self,
        node_id: NodeId,
        event_type: &str,
        phase: EventPhase,
    ) -> Vec<&EventListenerEntry> {
        self.listeners
            .get(&node_id)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|e| {
                        !e.removed
                            && e.event_type == event_type
                            && match phase {
                                EventPhase::Capturing => e.options.capture,
                                EventPhase::AtTarget => true,
                                EventPhase::Bubbling => !e.options.capture,
                                EventPhase::None => false,
                            }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Clean up removed listeners.
    pub fn cleanup(&mut self) {
        for listeners in self.listeners.values_mut() {
            listeners.retain(|e| !e.removed);
        }
        self.listeners.retain(|_, v| !v.is_empty());
    }
}

// ==================== Pointer Lock ====================

/// Pointer lock state for FPS games.
#[derive(Debug, Default)]
pub struct PointerLockState {
    /// Element that has pointer lock.
    locked_element: Option<NodeId>,
    /// Pending lock request.
    pending_lock: Option<NodeId>,
    /// Raw movement deltas while locked.
    movement_x: f64,
    movement_y: f64,
}

impl PointerLockState {
    /// Create a new pointer lock state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request pointer lock for an element.
    pub fn request_lock(&mut self, element: NodeId) -> bool {
        if self.locked_element.is_some() {
            return false;
        }
        self.pending_lock = Some(element);
        true
    }

    /// Confirm pointer lock.
    pub fn confirm_lock(&mut self) -> Option<NodeId> {
        if let Some(element) = self.pending_lock.take() {
            self.locked_element = Some(element);
            debug!("Pointer locked to {:?}", element);
            Some(element)
        } else {
            None
        }
    }

    /// Exit pointer lock.
    pub fn exit_lock(&mut self) -> Option<NodeId> {
        let old = self.locked_element.take();
        self.pending_lock = None;
        if old.is_some() {
            debug!("Pointer lock released");
        }
        old
    }

    /// Check if pointer is locked.
    pub fn is_locked(&self) -> bool {
        self.locked_element.is_some()
    }

    /// Get the locked element.
    pub fn locked_element(&self) -> Option<NodeId> {
        self.locked_element
    }

    /// Update movement while locked.
    pub fn update_movement(&mut self, dx: f64, dy: f64) {
        self.movement_x += dx;
        self.movement_y += dy;
    }

    /// Get and reset movement.
    pub fn get_movement(&mut self) -> (f64, f64) {
        let result = (self.movement_x, self.movement_y);
        self.movement_x = 0.0;
        self.movement_y = 0.0;
        result
    }
}

// ==================== Wheel Event ====================

/// Wheel event delta mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WheelDeltaMode {
    #[default]
    Pixel = 0,
    Line = 1,
    Page = 2,
}

/// Wheel event data.
#[derive(Debug, Clone, Default)]
pub struct WheelEventData {
    // Mouse event data
    pub client_x: f64,
    pub client_y: f64,
    pub screen_x: f64,
    pub screen_y: f64,
    pub ctrl_key: bool,
    pub alt_key: bool,
    pub shift_key: bool,
    pub meta_key: bool,
    
    // Wheel-specific
    pub delta_x: f64,
    pub delta_y: f64,
    pub delta_z: f64,
    pub delta_mode: WheelDeltaMode,
}

// ==================== Comprehensive Event Data ====================

/// All possible event data types.
#[derive(Debug, Clone)]
pub enum ExtendedEventData {
    /// Basic mouse event.
    Mouse {
        client_x: f64,
        client_y: f64,
        screen_x: f64,
        screen_y: f64,
        offset_x: f64,
        offset_y: f64,
        button: i16,
        buttons: u16,
        ctrl_key: bool,
        alt_key: bool,
        shift_key: bool,
        meta_key: bool,
    },
    /// Keyboard event.
    Keyboard {
        key: String,
        code: String,
        repeat: bool,
        ctrl_key: bool,
        alt_key: bool,
        shift_key: bool,
        meta_key: bool,
        location: u32,
    },
    /// Focus event.
    Focus {
        related_target: Option<NodeId>,
    },
    /// Input event.
    Input {
        data: Option<String>,
        input_type: String,
        is_composing: bool,
    },
    /// Pointer event.
    Pointer(PointerEventData),
    /// Touch event.
    Touch(TouchEventData),
    /// Wheel event.
    Wheel(WheelEventData),
    /// Drag event.
    Drag(DragEventData),
    /// Transition event.
    Transition(TransitionEventData),
    /// Animation event.
    Animation(AnimationEventData),
    /// Message event.
    Message(MessageEventData),
    /// Generic event (no additional data).
    Generic,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raf_scheduler() {
        let mut scheduler = RafScheduler::new();
        
        let id1 = scheduler.request("callback1()".to_string());
        let id2 = scheduler.request("callback2()".to_string());
        
        assert!(scheduler.has_pending());
        
        scheduler.cancel(id2);
        
        let callbacks = scheduler.tick();
        assert_eq!(callbacks.len(), 1);
        assert_eq!(callbacks[0].0, id1);
    }

    #[test]
    fn test_hover_tracker() {
        let mut tracker = HoverTracker::new();
        
        let node1 = NodeId::new(1);
        let node2 = NodeId::new(2);
        let node3 = NodeId::new(3);
        
        // Initial hover
        let (entered, left) = tracker.update(vec![node1, node2]);
        assert_eq!(entered.len(), 2);
        assert!(left.is_empty());
        
        // Move deeper
        let (entered, left) = tracker.update(vec![node1, node2, node3]);
        assert_eq!(entered, vec![node3]);
        assert!(left.is_empty());
        
        // Move to sibling
        let node4 = NodeId::new(4);
        let (entered, left) = tracker.update(vec![node1, node4]);
        assert_eq!(entered, vec![node4]);
        assert!(left.contains(&node2));
        assert!(left.contains(&node3));
    }

    #[test]
    fn test_focus_manager() {
        let mut manager = FocusManager::new();
        
        let node1 = NodeId::new(1);
        let node2 = NodeId::new(2);
        let node3 = NodeId::new(3);
        
        // Set focus via keyboard
        let (old, new) = manager.set_focus(Some(node1), true);
        assert!(old.is_none());
        assert_eq!(new, Some(node1));
        assert!(manager.is_focus_visible());
        
        // Set focus via mouse
        manager.set_focus(Some(node2), false);
        assert!(!manager.is_focus_visible());
        
        // Update tab order
        manager.update_tab_order(vec![
            FocusableElement { node_id: node1, tab_index: 0, is_disabled: false, is_inert: false },
            FocusableElement { node_id: node2, tab_index: 0, is_disabled: false, is_inert: false },
            FocusableElement { node_id: node3, tab_index: 1, is_disabled: false, is_inert: false },
        ]);
        
        // Tab forward (node3 has higher tabindex so comes first)
        manager.set_focus(Some(node3), true);
        let next = manager.move_next();
        assert_eq!(next, Some(node1));
    }

    #[test]
    fn test_event_dispatcher() {
        let mut dispatcher = EventDispatcher::new();
        let node = NodeId::new(1);
        
        let id1 = dispatcher.add_listener(
            node,
            "click",
            "handleClick()",
            EventListenerOptions { capture: false, ..Default::default() },
        );
        
        let _id2 = dispatcher.add_listener(
            node,
            "click",
            "handleClickCapture()",
            EventListenerOptions { capture: true, ..Default::default() },
        );
        
        // Get capture phase listeners
        let capture = dispatcher.get_listeners(node, "click", EventPhase::Capturing);
        assert_eq!(capture.len(), 1);
        assert_eq!(capture[0].callback, "handleClickCapture()");
        
        // Get bubble phase listeners
        let bubble = dispatcher.get_listeners(node, "click", EventPhase::Bubbling);
        assert_eq!(bubble.len(), 1);
        assert_eq!(bubble[0].callback, "handleClick()");
        
        // Remove listener
        dispatcher.remove_listener(node, id1);
        dispatcher.cleanup();
        
        let bubble = dispatcher.get_listeners(node, "click", EventPhase::Bubbling);
        assert!(bubble.is_empty());
    }

    #[test]
    fn test_pointer_event_from_mouse() {
        let pointer = PointerEventData::from_mouse(
            100.0, 200.0,
            0, 1,
            (false, false, true, false), // shift
        );
        
        assert_eq!(pointer.client_x, 100.0);
        assert_eq!(pointer.client_y, 200.0);
        assert_eq!(pointer.pointer_type, PointerType::Mouse);
        assert!(pointer.is_primary);
        assert!(pointer.shift_key);
        assert!(!pointer.ctrl_key);
    }

    #[test]
    fn test_data_transfer() {
        let mut dt = DataTransfer::new();
        
        dt.set_data("text/plain", "Hello");
        dt.set_data("text/html", "<b>Hello</b>");
        
        assert_eq!(dt.types.len(), 2);
        assert_eq!(dt.get_data("text/plain"), Some(&"Hello".to_string()));
        
        dt.clear_data(Some("text/plain"));
        assert!(dt.get_data("text/plain").is_none());
        assert_eq!(dt.types.len(), 1);
    }

    #[test]
    fn test_pointer_lock() {
        let mut lock = PointerLockState::new();
        let element = NodeId::new(1);
        
        assert!(!lock.is_locked());
        
        // Request lock
        assert!(lock.request_lock(element));
        assert!(!lock.is_locked()); // Not confirmed yet
        
        // Confirm
        let locked = lock.confirm_lock();
        assert_eq!(locked, Some(element));
        assert!(lock.is_locked());
        
        // Can't request while locked
        let other = NodeId::new(2);
        assert!(!lock.request_lock(other));
        
        // Exit
        let exited = lock.exit_lock();
        assert_eq!(exited, Some(element));
        assert!(!lock.is_locked());
    }
}

