//! # Input Events
//!
//! Input event types for mouse, keyboard, and focus handling.
//! These events are translated from platform-specific input (e.g., Win32 messages)
//! into a platform-agnostic representation.

use std::collections::HashSet;

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// Primary button (usually left).
    Primary,
    /// Secondary button (usually right).
    Secondary,
    /// Auxiliary button (usually middle/wheel).
    Auxiliary,
    /// Fourth button (browser back).
    Back,
    /// Fifth button (browser forward).
    Forward,
}

impl MouseButton {
    /// Get the button index (for JavaScript MouseEvent.button).
    pub fn button_index(&self) -> i16 {
        match self {
            MouseButton::Primary => 0,
            MouseButton::Auxiliary => 1,
            MouseButton::Secondary => 2,
            MouseButton::Back => 3,
            MouseButton::Forward => 4,
        }
    }

    /// Get the buttons mask (for JavaScript MouseEvent.buttons).
    pub fn button_mask(&self) -> u16 {
        match self {
            MouseButton::Primary => 1,
            MouseButton::Secondary => 2,
            MouseButton::Auxiliary => 4,
            MouseButton::Back => 8,
            MouseButton::Forward => 16,
        }
    }
}

/// Keyboard modifier keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool, // Windows/Command key
}

impl Modifiers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    pub fn with_alt(mut self) -> Self {
        self.alt = true;
        self
    }

    pub fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

    pub fn with_meta(mut self) -> Self {
        self.meta = true;
        self
    }

    /// Check if any modifier is pressed.
    pub fn any(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }

    /// Check if no modifiers are pressed.
    pub fn none(&self) -> bool {
        !self.any()
    }
}

/// Point in 2D space.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn zero() -> Self {
        Self::default()
    }
}

/// Mouse event data.
#[derive(Debug, Clone)]
pub struct MouseEvent {
    /// Event type.
    pub event_type: MouseEventType,
    /// Position relative to the view.
    pub position: Point,
    /// Position relative to the screen.
    pub screen_position: Point,
    /// Which button triggered the event (for button events).
    pub button: MouseButton,
    /// Currently pressed buttons (bitmask).
    pub buttons: u16,
    /// Modifier keys held during the event.
    pub modifiers: Modifiers,
    /// Click count (1 for single click, 2 for double click, etc.).
    pub click_count: u32,
    /// Delta for wheel events.
    pub delta: Point,
    /// Timestamp in milliseconds.
    pub timestamp: u64,
}

/// Mouse event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventType {
    /// Mouse button pressed down.
    MouseDown,
    /// Mouse button released.
    MouseUp,
    /// Mouse moved.
    MouseMove,
    /// Mouse entered the view.
    MouseEnter,
    /// Mouse left the view.
    MouseLeave,
    /// Mouse wheel scrolled.
    Wheel,
    /// Context menu requested (right-click or menu key).
    ContextMenu,
}

impl MouseEvent {
    /// Create a new mouse event.
    pub fn new(event_type: MouseEventType, position: Point) -> Self {
        Self {
            event_type,
            position,
            screen_position: position,
            button: MouseButton::Primary,
            buttons: 0,
            modifiers: Modifiers::default(),
            click_count: 1,
            delta: Point::zero(),
            timestamp: 0,
        }
    }

    /// Set the button.
    pub fn with_button(mut self, button: MouseButton) -> Self {
        self.button = button;
        self
    }

    /// Set the buttons bitmask.
    pub fn with_buttons(mut self, buttons: u16) -> Self {
        self.buttons = buttons;
        self
    }

    /// Set modifiers.
    pub fn with_modifiers(mut self, modifiers: Modifiers) -> Self {
        self.modifiers = modifiers;
        self
    }

    /// Set click count.
    pub fn with_click_count(mut self, count: u32) -> Self {
        self.click_count = count;
        self
    }

    /// Set wheel delta.
    pub fn with_delta(mut self, delta: Point) -> Self {
        self.delta = delta;
        self
    }

    /// Set timestamp.
    pub fn with_timestamp(mut self, timestamp: u64) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// Set screen position.
    pub fn with_screen_position(mut self, pos: Point) -> Self {
        self.screen_position = pos;
        self
    }

    /// Check if this is a click event (mousedown followed by mouseup).
    pub fn is_click(&self) -> bool {
        self.event_type == MouseEventType::MouseUp
    }
}

/// Virtual key codes (subset of common keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum KeyCode {
    // Letters
    KeyA = 0x41,
    KeyB = 0x42,
    KeyC = 0x43,
    KeyD = 0x44,
    KeyE = 0x45,
    KeyF = 0x46,
    KeyG = 0x47,
    KeyH = 0x48,
    KeyI = 0x49,
    KeyJ = 0x4A,
    KeyK = 0x4B,
    KeyL = 0x4C,
    KeyM = 0x4D,
    KeyN = 0x4E,
    KeyO = 0x4F,
    KeyP = 0x50,
    KeyQ = 0x51,
    KeyR = 0x52,
    KeyS = 0x53,
    KeyT = 0x54,
    KeyU = 0x55,
    KeyV = 0x56,
    KeyW = 0x57,
    KeyX = 0x58,
    KeyY = 0x59,
    KeyZ = 0x5A,

    // Numbers
    Digit0 = 0x30,
    Digit1 = 0x31,
    Digit2 = 0x32,
    Digit3 = 0x33,
    Digit4 = 0x34,
    Digit5 = 0x35,
    Digit6 = 0x36,
    Digit7 = 0x37,
    Digit8 = 0x38,
    Digit9 = 0x39,

    // Function keys
    F1 = 0x70,
    F2 = 0x71,
    F3 = 0x72,
    F4 = 0x73,
    F5 = 0x74,
    F6 = 0x75,
    F7 = 0x76,
    F8 = 0x77,
    F9 = 0x78,
    F10 = 0x79,
    F11 = 0x7A,
    F12 = 0x7B,

    // Navigation
    Escape = 0x1B,
    Tab = 0x09,
    CapsLock = 0x14,
    Backspace = 0x08,
    Enter = 0x0D,
    Space = 0x20,
    Insert = 0x2D,
    Delete = 0x2E,
    Home = 0x24,
    End = 0x23,
    PageUp = 0x21,
    PageDown = 0x22,

    // Arrows
    ArrowLeft = 0x25,
    ArrowUp = 0x26,
    ArrowRight = 0x27,
    ArrowDown = 0x28,

    // Modifiers
    ShiftLeft = 0xA0,
    ShiftRight = 0xA1,
    ControlLeft = 0xA2,
    ControlRight = 0xA3,
    AltLeft = 0xA4,
    AltRight = 0xA5,
    MetaLeft = 0x5B,
    MetaRight = 0x5C,

    // Punctuation
    Minus = 0xBD,
    Equal = 0xBB,
    BracketLeft = 0xDB,
    BracketRight = 0xDD,
    Backslash = 0xDC,
    Semicolon = 0xBA,
    Quote = 0xDE,
    Comma = 0xBC,
    Period = 0xBE,
    Slash = 0xBF,
    Backquote = 0xC0,

    // Unknown key
    Unknown = 0,
}

impl KeyCode {
    /// Try to convert from a Win32 virtual key code.
    pub fn from_vk(vk: u32) -> Self {
        match vk {
            0x41 => KeyCode::KeyA,
            0x42 => KeyCode::KeyB,
            0x43 => KeyCode::KeyC,
            0x44 => KeyCode::KeyD,
            0x45 => KeyCode::KeyE,
            0x46 => KeyCode::KeyF,
            0x47 => KeyCode::KeyG,
            0x48 => KeyCode::KeyH,
            0x49 => KeyCode::KeyI,
            0x4A => KeyCode::KeyJ,
            0x4B => KeyCode::KeyK,
            0x4C => KeyCode::KeyL,
            0x4D => KeyCode::KeyM,
            0x4E => KeyCode::KeyN,
            0x4F => KeyCode::KeyO,
            0x50 => KeyCode::KeyP,
            0x51 => KeyCode::KeyQ,
            0x52 => KeyCode::KeyR,
            0x53 => KeyCode::KeyS,
            0x54 => KeyCode::KeyT,
            0x55 => KeyCode::KeyU,
            0x56 => KeyCode::KeyV,
            0x57 => KeyCode::KeyW,
            0x58 => KeyCode::KeyX,
            0x59 => KeyCode::KeyY,
            0x5A => KeyCode::KeyZ,
            0x30..=0x39 => unsafe { std::mem::transmute::<u32, KeyCode>(vk) },
            0x70..=0x7B => unsafe { std::mem::transmute::<u32, KeyCode>(vk) },
            0x1B => KeyCode::Escape,
            0x09 => KeyCode::Tab,
            0x14 => KeyCode::CapsLock,
            0x08 => KeyCode::Backspace,
            0x0D => KeyCode::Enter,
            0x20 => KeyCode::Space,
            0x2D => KeyCode::Insert,
            0x2E => KeyCode::Delete,
            0x24 => KeyCode::Home,
            0x23 => KeyCode::End,
            0x21 => KeyCode::PageUp,
            0x22 => KeyCode::PageDown,
            0x25 => KeyCode::ArrowLeft,
            0x26 => KeyCode::ArrowUp,
            0x27 => KeyCode::ArrowRight,
            0x28 => KeyCode::ArrowDown,
            0xA0 => KeyCode::ShiftLeft,
            0xA1 => KeyCode::ShiftRight,
            0xA2 => KeyCode::ControlLeft,
            0xA3 => KeyCode::ControlRight,
            0xA4 => KeyCode::AltLeft,
            0xA5 => KeyCode::AltRight,
            0x5B => KeyCode::MetaLeft,
            0x5C => KeyCode::MetaRight,
            // WM_KEYDOWN reports Shift, Ctrl and Alt with the GENERIC codes; the
            // left/right codes above come only from GetKeyState and hooks. Without
            // these, a real keyboard never sets a modifier and every chord is dead.
            0x10 => KeyCode::ShiftLeft,
            0x11 => KeyCode::ControlLeft,
            0x12 => KeyCode::AltLeft,
            _ => KeyCode::Unknown,
        }
    }

    /// Get the key string for KeyboardEvent.key.
    pub fn key_string(&self, shift: bool) -> &'static str {
        match self {
            KeyCode::KeyA => {
                if shift {
                    "A"
                } else {
                    "a"
                }
            }
            KeyCode::KeyB => {
                if shift {
                    "B"
                } else {
                    "b"
                }
            }
            KeyCode::KeyC => {
                if shift {
                    "C"
                } else {
                    "c"
                }
            }
            KeyCode::KeyD => {
                if shift {
                    "D"
                } else {
                    "d"
                }
            }
            KeyCode::KeyE => {
                if shift {
                    "E"
                } else {
                    "e"
                }
            }
            KeyCode::KeyF => {
                if shift {
                    "F"
                } else {
                    "f"
                }
            }
            KeyCode::KeyG => {
                if shift {
                    "G"
                } else {
                    "g"
                }
            }
            KeyCode::KeyH => {
                if shift {
                    "H"
                } else {
                    "h"
                }
            }
            KeyCode::KeyI => {
                if shift {
                    "I"
                } else {
                    "i"
                }
            }
            KeyCode::KeyJ => {
                if shift {
                    "J"
                } else {
                    "j"
                }
            }
            KeyCode::KeyK => {
                if shift {
                    "K"
                } else {
                    "k"
                }
            }
            KeyCode::KeyL => {
                if shift {
                    "L"
                } else {
                    "l"
                }
            }
            KeyCode::KeyM => {
                if shift {
                    "M"
                } else {
                    "m"
                }
            }
            KeyCode::KeyN => {
                if shift {
                    "N"
                } else {
                    "n"
                }
            }
            KeyCode::KeyO => {
                if shift {
                    "O"
                } else {
                    "o"
                }
            }
            KeyCode::KeyP => {
                if shift {
                    "P"
                } else {
                    "p"
                }
            }
            KeyCode::KeyQ => {
                if shift {
                    "Q"
                } else {
                    "q"
                }
            }
            KeyCode::KeyR => {
                if shift {
                    "R"
                } else {
                    "r"
                }
            }
            KeyCode::KeyS => {
                if shift {
                    "S"
                } else {
                    "s"
                }
            }
            KeyCode::KeyT => {
                if shift {
                    "T"
                } else {
                    "t"
                }
            }
            KeyCode::KeyU => {
                if shift {
                    "U"
                } else {
                    "u"
                }
            }
            KeyCode::KeyV => {
                if shift {
                    "V"
                } else {
                    "v"
                }
            }
            KeyCode::KeyW => {
                if shift {
                    "W"
                } else {
                    "w"
                }
            }
            KeyCode::KeyX => {
                if shift {
                    "X"
                } else {
                    "x"
                }
            }
            KeyCode::KeyY => {
                if shift {
                    "Y"
                } else {
                    "y"
                }
            }
            KeyCode::KeyZ => {
                if shift {
                    "Z"
                } else {
                    "z"
                }
            }
            KeyCode::Digit0 => {
                if shift {
                    ")"
                } else {
                    "0"
                }
            }
            KeyCode::Digit1 => {
                if shift {
                    "!"
                } else {
                    "1"
                }
            }
            KeyCode::Digit2 => {
                if shift {
                    "@"
                } else {
                    "2"
                }
            }
            KeyCode::Digit3 => {
                if shift {
                    "#"
                } else {
                    "3"
                }
            }
            KeyCode::Digit4 => {
                if shift {
                    "$"
                } else {
                    "4"
                }
            }
            KeyCode::Digit5 => {
                if shift {
                    "%"
                } else {
                    "5"
                }
            }
            KeyCode::Digit6 => {
                if shift {
                    "^"
                } else {
                    "6"
                }
            }
            KeyCode::Digit7 => {
                if shift {
                    "&"
                } else {
                    "7"
                }
            }
            KeyCode::Digit8 => {
                if shift {
                    "*"
                } else {
                    "8"
                }
            }
            KeyCode::Digit9 => {
                if shift {
                    "("
                } else {
                    "9"
                }
            }
            KeyCode::F1 => "F1",
            KeyCode::F2 => "F2",
            KeyCode::F3 => "F3",
            KeyCode::F4 => "F4",
            KeyCode::F5 => "F5",
            KeyCode::F6 => "F6",
            KeyCode::F7 => "F7",
            KeyCode::F8 => "F8",
            KeyCode::F9 => "F9",
            KeyCode::F10 => "F10",
            KeyCode::F11 => "F11",
            KeyCode::F12 => "F12",
            KeyCode::Escape => "Escape",
            KeyCode::Tab => "Tab",
            KeyCode::CapsLock => "CapsLock",
            KeyCode::Backspace => "Backspace",
            KeyCode::Enter => "Enter",
            KeyCode::Space => " ",
            KeyCode::Insert => "Insert",
            KeyCode::Delete => "Delete",
            KeyCode::Home => "Home",
            KeyCode::End => "End",
            KeyCode::PageUp => "PageUp",
            KeyCode::PageDown => "PageDown",
            KeyCode::ArrowLeft => "ArrowLeft",
            KeyCode::ArrowUp => "ArrowUp",
            KeyCode::ArrowRight => "ArrowRight",
            KeyCode::ArrowDown => "ArrowDown",
            KeyCode::ShiftLeft | KeyCode::ShiftRight => "Shift",
            KeyCode::ControlLeft | KeyCode::ControlRight => "Control",
            KeyCode::AltLeft | KeyCode::AltRight => "Alt",
            KeyCode::MetaLeft | KeyCode::MetaRight => "Meta",
            KeyCode::Minus => {
                if shift {
                    "_"
                } else {
                    "-"
                }
            }
            KeyCode::Equal => {
                if shift {
                    "+"
                } else {
                    "="
                }
            }
            KeyCode::BracketLeft => {
                if shift {
                    "{"
                } else {
                    "["
                }
            }
            KeyCode::BracketRight => {
                if shift {
                    "}"
                } else {
                    "]"
                }
            }
            KeyCode::Backslash => {
                if shift {
                    "|"
                } else {
                    "\\"
                }
            }
            KeyCode::Semicolon => {
                if shift {
                    ":"
                } else {
                    ";"
                }
            }
            KeyCode::Quote => {
                if shift {
                    "\""
                } else {
                    "'"
                }
            }
            KeyCode::Comma => {
                if shift {
                    "<"
                } else {
                    ","
                }
            }
            KeyCode::Period => {
                if shift {
                    ">"
                } else {
                    "."
                }
            }
            KeyCode::Slash => {
                if shift {
                    "?"
                } else {
                    "/"
                }
            }
            KeyCode::Backquote => {
                if shift {
                    "~"
                } else {
                    "`"
                }
            }
            KeyCode::Unknown => "Unidentified",
        }
    }

    /// Get the code string for KeyboardEvent.code.
    pub fn code_string(&self) -> &'static str {
        match self {
            KeyCode::KeyA => "KeyA",
            KeyCode::KeyB => "KeyB",
            KeyCode::KeyC => "KeyC",
            KeyCode::KeyD => "KeyD",
            KeyCode::KeyE => "KeyE",
            KeyCode::KeyF => "KeyF",
            KeyCode::KeyG => "KeyG",
            KeyCode::KeyH => "KeyH",
            KeyCode::KeyI => "KeyI",
            KeyCode::KeyJ => "KeyJ",
            KeyCode::KeyK => "KeyK",
            KeyCode::KeyL => "KeyL",
            KeyCode::KeyM => "KeyM",
            KeyCode::KeyN => "KeyN",
            KeyCode::KeyO => "KeyO",
            KeyCode::KeyP => "KeyP",
            KeyCode::KeyQ => "KeyQ",
            KeyCode::KeyR => "KeyR",
            KeyCode::KeyS => "KeyS",
            KeyCode::KeyT => "KeyT",
            KeyCode::KeyU => "KeyU",
            KeyCode::KeyV => "KeyV",
            KeyCode::KeyW => "KeyW",
            KeyCode::KeyX => "KeyX",
            KeyCode::KeyY => "KeyY",
            KeyCode::KeyZ => "KeyZ",
            KeyCode::Digit0 => "Digit0",
            KeyCode::Digit1 => "Digit1",
            KeyCode::Digit2 => "Digit2",
            KeyCode::Digit3 => "Digit3",
            KeyCode::Digit4 => "Digit4",
            KeyCode::Digit5 => "Digit5",
            KeyCode::Digit6 => "Digit6",
            KeyCode::Digit7 => "Digit7",
            KeyCode::Digit8 => "Digit8",
            KeyCode::Digit9 => "Digit9",
            KeyCode::F1 => "F1",
            KeyCode::F2 => "F2",
            KeyCode::F3 => "F3",
            KeyCode::F4 => "F4",
            KeyCode::F5 => "F5",
            KeyCode::F6 => "F6",
            KeyCode::F7 => "F7",
            KeyCode::F8 => "F8",
            KeyCode::F9 => "F9",
            KeyCode::F10 => "F10",
            KeyCode::F11 => "F11",
            KeyCode::F12 => "F12",
            KeyCode::Escape => "Escape",
            KeyCode::Tab => "Tab",
            KeyCode::CapsLock => "CapsLock",
            KeyCode::Backspace => "Backspace",
            KeyCode::Enter => "Enter",
            KeyCode::Space => "Space",
            KeyCode::Insert => "Insert",
            KeyCode::Delete => "Delete",
            KeyCode::Home => "Home",
            KeyCode::End => "End",
            KeyCode::PageUp => "PageUp",
            KeyCode::PageDown => "PageDown",
            KeyCode::ArrowLeft => "ArrowLeft",
            KeyCode::ArrowUp => "ArrowUp",
            KeyCode::ArrowRight => "ArrowRight",
            KeyCode::ArrowDown => "ArrowDown",
            KeyCode::ShiftLeft => "ShiftLeft",
            KeyCode::ShiftRight => "ShiftRight",
            KeyCode::ControlLeft => "ControlLeft",
            KeyCode::ControlRight => "ControlRight",
            KeyCode::AltLeft => "AltLeft",
            KeyCode::AltRight => "AltRight",
            KeyCode::MetaLeft => "MetaLeft",
            KeyCode::MetaRight => "MetaRight",
            KeyCode::Minus => "Minus",
            KeyCode::Equal => "Equal",
            KeyCode::BracketLeft => "BracketLeft",
            KeyCode::BracketRight => "BracketRight",
            KeyCode::Backslash => "Backslash",
            KeyCode::Semicolon => "Semicolon",
            KeyCode::Quote => "Quote",
            KeyCode::Comma => "Comma",
            KeyCode::Period => "Period",
            KeyCode::Slash => "Slash",
            KeyCode::Backquote => "Backquote",
            KeyCode::Unknown => "Unidentified",
        }
    }
}

/// Keyboard event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventType {
    /// Key pressed down.
    KeyDown,
    /// Key released.
    KeyUp,
    /// Character input (after key processing).
    Input,
}

/// Keyboard event data.
#[derive(Debug, Clone)]
pub struct KeyEvent {
    /// Event type.
    pub event_type: KeyEventType,
    /// Virtual key code.
    pub key_code: KeyCode,
    /// The key value (for Input events, this is the character).
    pub key: String,
    /// Physical key code (KeyboardEvent.code).
    pub code: String,
    /// Modifier keys held during the event.
    pub modifiers: Modifiers,
    /// Whether this is a repeat event.
    pub repeat: bool,
    /// Timestamp in milliseconds.
    pub timestamp: u64,
}

impl KeyEvent {
    /// Create a new key event.
    pub fn new(event_type: KeyEventType, key_code: KeyCode, modifiers: Modifiers) -> Self {
        Self {
            event_type,
            key: key_code.key_string(modifiers.shift).to_string(),
            code: key_code.code_string().to_string(),
            key_code,
            modifiers,
            repeat: false,
            timestamp: 0,
        }
    }

    /// Create an input event with a specific character.
    pub fn input(ch: char) -> Self {
        Self {
            event_type: KeyEventType::Input,
            key_code: KeyCode::Unknown,
            key: ch.to_string(),
            code: String::new(),
            modifiers: Modifiers::default(),
            repeat: false,
            timestamp: 0,
        }
    }

    /// Set repeat flag.
    pub fn with_repeat(mut self, repeat: bool) -> Self {
        self.repeat = repeat;
        self
    }

    /// Set timestamp.
    pub fn with_timestamp(mut self, timestamp: u64) -> Self {
        self.timestamp = timestamp;
        self
    }
}

/// Focus event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusEventType {
    /// Element gained focus.
    Focus,
    /// Element lost focus.
    Blur,
    /// Focus is moving into the element (bubbles).
    FocusIn,
    /// Focus is moving out of the element (bubbles).
    FocusOut,
}

/// Focus event data.
#[derive(Debug, Clone)]
pub struct FocusEvent {
    /// Event type.
    pub event_type: FocusEventType,
    /// The element receiving or losing focus.
    pub target_node_id: Option<u64>,
    /// The related element (the one losing or gaining focus).
    pub related_target_node_id: Option<u64>,
    /// Timestamp in milliseconds.
    pub timestamp: u64,
}

impl FocusEvent {
    /// Create a new focus event.
    pub fn new(event_type: FocusEventType) -> Self {
        Self {
            event_type,
            target_node_id: None,
            related_target_node_id: None,
            timestamp: 0,
        }
    }

    /// Set the target node.
    pub fn with_target(mut self, node_id: u64) -> Self {
        self.target_node_id = Some(node_id);
        self
    }

    /// Set the related target node.
    pub fn with_related_target(mut self, node_id: u64) -> Self {
        self.related_target_node_id = Some(node_id);
        self
    }

    /// Set timestamp.
    pub fn with_timestamp(mut self, timestamp: u64) -> Self {
        self.timestamp = timestamp;
        self
    }
}

/// Unified input event type.
#[derive(Debug, Clone)]
pub enum InputEvent {
    Mouse(MouseEvent),
    Key(KeyEvent),
    Focus(FocusEvent),
}

/// Track currently pressed keys for repeat detection.
#[derive(Debug, Default)]
pub struct KeyboardState {
    pressed_keys: HashSet<KeyCode>,
    modifiers: Modifiers,
}

impl KeyboardState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Handle a key down event, returns true if this is a repeat.
    pub fn key_down(&mut self, key_code: KeyCode) -> bool {
        let is_repeat = self.pressed_keys.contains(&key_code);
        self.pressed_keys.insert(key_code);
        self.update_modifiers(key_code, true);
        is_repeat
    }

    /// Handle a key up event.
    pub fn key_up(&mut self, key_code: KeyCode) {
        self.pressed_keys.remove(&key_code);
        self.update_modifiers(key_code, false);
    }

    /// Get current modifiers.
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Check if a key is currently pressed.
    pub fn is_pressed(&self, key_code: KeyCode) -> bool {
        self.pressed_keys.contains(&key_code)
    }

    fn update_modifiers(&mut self, key_code: KeyCode, pressed: bool) {
        match key_code {
            KeyCode::ShiftLeft | KeyCode::ShiftRight => self.modifiers.shift = pressed,
            KeyCode::ControlLeft | KeyCode::ControlRight => self.modifiers.ctrl = pressed,
            KeyCode::AltLeft | KeyCode::AltRight => self.modifiers.alt = pressed,
            KeyCode::MetaLeft | KeyCode::MetaRight => self.modifiers.meta = pressed,
            _ => {}
        }
    }
}

/// Track mouse button state.
#[derive(Debug, Default)]
pub struct MouseState {
    pub position: Point,
    pub buttons: u16,
}

impl MouseState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update position.
    pub fn set_position(&mut self, pos: Point) {
        self.position = pos;
    }

    /// Handle button down.
    pub fn button_down(&mut self, button: MouseButton) {
        self.buttons |= button.button_mask();
    }

    /// Handle button up.
    pub fn button_up(&mut self, button: MouseButton) {
        self.buttons &= !button.button_mask();
    }

    /// Check if a button is pressed.
    pub fn is_pressed(&self, button: MouseButton) -> bool {
        (self.buttons & button.button_mask()) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mouse_button_indices() {
        assert_eq!(MouseButton::Primary.button_index(), 0);
        assert_eq!(MouseButton::Auxiliary.button_index(), 1);
        assert_eq!(MouseButton::Secondary.button_index(), 2);
    }

    #[test]
    fn test_modifiers() {
        let mods = Modifiers::new().with_ctrl().with_shift();
        assert!(mods.ctrl);
        assert!(mods.shift);
        assert!(!mods.alt);
        assert!(!mods.meta);
        assert!(mods.any());
    }

    #[test]
    fn test_key_code_from_vk() {
        assert_eq!(KeyCode::from_vk(0x41), KeyCode::KeyA);
        assert_eq!(KeyCode::from_vk(0x1B), KeyCode::Escape);
        assert_eq!(KeyCode::from_vk(0x0D), KeyCode::Enter);
    }

    #[test]
    fn the_generic_modifier_codes_a_real_keyboard_sends_set_the_modifiers() {
        let mut state = KeyboardState::new();
        // VK_CONTROL (0x11), VK_SHIFT (0x10), VK_MENU (0x12): what WM_KEYDOWN carries.
        state.key_down(KeyCode::from_vk(0x11));
        assert!(state.modifiers().ctrl);
        state.key_down(KeyCode::from_vk(0x10));
        assert!(state.modifiers().shift);
        state.key_down(KeyCode::from_vk(0x12));
        assert!(state.modifiers().alt);
        state.key_up(KeyCode::from_vk(0x11));
        assert!(!state.modifiers().ctrl);
    }

    #[test]
    fn test_key_string() {
        assert_eq!(KeyCode::KeyA.key_string(false), "a");
        assert_eq!(KeyCode::KeyA.key_string(true), "A");
        assert_eq!(KeyCode::Enter.key_string(false), "Enter");
    }

    #[test]
    fn test_keyboard_state() {
        let mut state = KeyboardState::new();

        // First press is not repeat
        assert!(!state.key_down(KeyCode::KeyA));
        assert!(state.is_pressed(KeyCode::KeyA));

        // Second press is repeat
        assert!(state.key_down(KeyCode::KeyA));

        // Release
        state.key_up(KeyCode::KeyA);
        assert!(!state.is_pressed(KeyCode::KeyA));
    }

    #[test]
    fn test_mouse_state() {
        let mut state = MouseState::new();

        state.button_down(MouseButton::Primary);
        assert!(state.is_pressed(MouseButton::Primary));
        assert!(!state.is_pressed(MouseButton::Secondary));

        state.button_up(MouseButton::Primary);
        assert!(!state.is_pressed(MouseButton::Primary));
    }
}
