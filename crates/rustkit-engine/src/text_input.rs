//! macOS text input (CH2.E3): marked text (a dead key or an IME composing),
//! commits, the Edit menu's commands and Cmd chords, as the content view's
//! `NSTextInputClient` and responder methods hand them to the page.
//!
//! The composition's events are script's (`document.__rkCompose` in
//! rustkit-bindings' `dom/editing.js`); the order is Chrome 143's, logged
//! by `tools/parity_oracle/composition_log.mjs`.

use crate::{arm_live_turn, end_live_turn, wgate, Engine, EngineViewId, RelayoutCause};
use rustkit_dom::forms::{keyboard, EditCommand, KeyPlatform};
use tracing::debug;

/// A focused text control's text-input state, for the platform's input
/// client: what is marked and selected (UTF-16 offsets into the value) and
/// where the caret is painted (viewport CSS px), so an IME's candidate
/// window sits at the caret.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextInputState {
    pub marked: Option<(usize, usize)>,
    pub selected: (usize, usize),
    pub caret: Option<TextInputCaret>,
}

/// The caret's rectangle in viewport CSS px.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextInputCaret {
    pub x: f32,
    pub y: f32,
    pub height: f32,
}

/// Win32 virtual keys the Cmd chords of a text field use (as the window
/// maps macOS keyCodes): Backspace, Delete and the arrows.
const CMD_EDIT_KEYS: [u32; 6] = [0x08, 0x2E, 0x25, 0x26, 0x27, 0x28];

impl Engine {
    /// The focused `<input>`/`<textarea>` of view `id`, if one has focus.
    fn focused_text_input(&self, id: EngineViewId) -> Option<usize> {
        let focused = self.views.get(&id)?.lay.focused_node.get()?.raw();
        (self.focused_is_text_control(id) && !self.is_editing_host(id, focused)).then_some(focused)
    }

    /// Run `__rkCompose` in the focused text control, then lay out what
    /// its listeners and the edit changed. `None` when no text control has
    /// focus or script answered nothing.
    fn compose(&mut self, id: EngineViewId, action: &str, text: &str) -> Option<serde_json::Value> {
        let node = self.focused_text_input(id)?;
        let view = self.views.get(&id)?;
        if view.control.stops_all() {
            return None;
        }
        let bindings = view.bindings.as_ref()?;
        let _turn = rustkit_js::DiagTurn::enter("live");
        let armed = arm_live_turn(&self.config, bindings);
        let report = bindings.user_compose(node, action, text).unwrap_or_else(|e| {
            debug!(?id, error = %e, "composition listener threw");
            None
        });
        let stopped = end_live_turn(&self.config, armed, bindings, "event:composition");
        if let (Some(stopped), Some(view)) = (stopped, self.views.get_mut(&id)) {
            view.script_log.push(stopped);
        }
        if action != "state" {
            if let Err(e) = self.flush_script_dom_writes(id, Some(RelayoutCause::Input)) {
                debug!(?id, error = %e, "relayout after composition failed");
            }
        }
        report
    }

    /// `setMarkedText:` — a dead key or an IME composing `text` in the
    /// focused text control: the first mark starts a composition over the
    /// selection, each mark replaces what is marked, an empty mark cancels.
    /// Whether anything happened.
    pub fn set_marked_text(&mut self, id: EngineViewId, text: &str) -> bool {
        let _hold = wgate::hold("key");
        self.compose(id, "mark", text)
            .is_some_and(|r| r["changed"] != serde_json::Value::Bool(false))
    }

    /// `insertText:` — commit `text`: in place of the marked text when a
    /// composition is open (compositionend), otherwise as typed text
    /// (`beforeinput`/`input` insertText; a contenteditable host types it).
    /// Whether anything changed.
    pub fn insert_text(&mut self, id: EngineViewId, text: &str) -> bool {
        let _hold = wgate::hold("key");
        if self.compose(id, "commit", text).is_some() {
            return true;
        }
        let Some(focused) = self.views.get(&id).and_then(|v| v.lay.focused_node.get()).map(|n| n.raw()) else {
            return false;
        };
        if self.is_editing_host(id, focused) {
            return self.editable_key(id, focused, 0, text, false);
        }
        if self.focused_text_input(id).is_none() || text.is_empty() {
            return false;
        }
        self.user_edit_command(id, focused, &EditCommand::InsertText(text.to_string()))
            .unwrap_or(false)
    }

    /// `insertText:replacementRange:` with nothing marked: `replace`
    /// (UTF-16 start, length) is the part of the focused field's value the
    /// text replaces, as the press-and-hold accent popover replaces the
    /// letter typed before it. `None` is [`Self::insert_text`].
    pub fn insert_text_replacing(&mut self, id: EngineViewId, text: &str, replace: Option<(usize, usize)>) -> bool {
        if let (Some((start, len)), Some(node)) = (replace, self.focused_text_input(id)) {
            let composing = self
                .compose(id, "state", "")
                .is_some_and(|r| !r["marked"].is_null());
            if !composing {
                if let Some(view) = self.views.get(&id) {
                    if let Some(state) = view.lay.edit_states.borrow().get(&node) {
                        let value = state.value();
                        let byte = |units: usize| -> usize {
                            let mut used = 0;
                            for (i, c) in value.char_indices() {
                                if used >= units {
                                    return i;
                                }
                                used += c.len_utf16();
                            }
                            value.len()
                        };
                        state.set_selection(byte(start), byte(start + len));
                    }
                }
            }
        }
        self.insert_text(id, text)
    }

    /// `unmarkText` — keep what is marked and end the composition.
    /// Whether a composition was open.
    pub fn unmark_text(&mut self, id: EngineViewId) -> bool {
        let _hold = wgate::hold("key");
        self.compose(id, "unmark", "")
            .is_some_and(|r| r["changed"] == serde_json::Value::Bool(true))
    }

    /// The focused text control's marked range, selection and caret, for
    /// the input client's `markedRange`, `selectedRange` and
    /// `firstRectForCharacterRange:`. `None` with no text control focused.
    pub fn text_input_state(&mut self, id: EngineViewId) -> Option<TextInputState> {
        let node = self.focused_text_input(id)?;
        let marked = self.compose(id, "state", "").and_then(|r| {
            let m = r.get("marked")?.as_array()?.clone();
            Some((m.first()?.as_u64()? as usize, m.get(1)?.as_u64()? as usize))
        });
        let view = self.views.get(&id)?;
        let selected = {
            let states = view.lay.edit_states.borrow();
            match states.get(&node) {
                Some(state) => {
                    let value = state.value();
                    let sel = state.selection().normalize();
                    let units = |b: usize| -> usize { value[..b.min(value.len())].encode_utf16().count() };
                    (units(sel.start), units(sel.end))
                }
                None => (0, 0),
            }
        };
        let (sx, sy) = view.scroll_offset;
        let caret = view.display_list.as_ref().and_then(|dl| {
            dl.commands.iter().rev().find_map(|c| match c {
                rustkit_layout::DisplayCommand::TextInput {
                    rect,
                    value,
                    font_size,
                    border_width,
                    focused: true,
                    caret_position: Some(pos),
                    font_family,
                    font_weight,
                    padding,
                    ..
                } => Some(control_caret(
                    *rect, value, *pos, *font_size, *border_width, *padding, font_family, *font_weight,
                )),
                _ => None,
            })
        });
        let caret = caret.map(|c| TextInputCaret { x: c.x - sx, y: c.y - sy, height: c.height });
        Some(TextInputState { marked, selected, caret })
    }

    /// A macOS Cmd chord (CH2.E3). The page hears its keydown first, with
    /// `metaKey`, as Chromium sends a key equivalent to the renderer before
    /// the menu. Whether the page claimed it: a listener cancelled the
    /// keydown, or the focused text field ran it as an edit (Cmd+A/C/X/V/Z,
    /// Cmd+Shift+Z, Cmd+arrows, Cmd+Backspace/Delete). A chord the page did
    /// not claim is the menu's (Cmd+T, Cmd+L, …): the window hands it on.
    pub fn handle_command_key(
        &mut self,
        id: EngineViewId,
        key_code: u32,
        key: &str,
        shift: bool,
        alt: bool,
    ) -> bool {
        let _hold = wgate::hold("key");
        let focused = self.views.get(&id).and_then(|v| v.lay.focused_node.get()).map(|n| n.raw());
        if !self.fire_key_mods(id, focused, "keydown", key_code, key, (false, shift, alt, true)) {
            return true;
        }
        let Some(node) = focused else {
            return false;
        };
        if !self.focused_is_text_control(id) {
            return false;
        }
        let platform = KeyPlatform::current();
        let command = keyboard::command_for_key(platform, key_code, key, true, shift, alt);
        if self.is_editing_host(id, node) {
            if command.is_none() && !CMD_EDIT_KEYS.contains(&key_code) {
                return false;
            }
            self.editable_key(id, node, key_code, key, true);
            return true;
        }
        if let Some(command) = command {
            if self.user_edit_command(id, node, &command).is_none() {
                self.text_key_default(id, node, key_code, key, true, shift, alt);
            }
            return true;
        }
        if CMD_EDIT_KEYS.contains(&key_code) {
            self.text_key_default(id, node, key_code, key, true, shift, alt);
            return true;
        }
        false
    }

    /// An Edit menu item (`copy:`, `cut:`, `paste:`, `undo:`, `redo:`,
    /// `selectAll:` reaching the content view): the command in the focused
    /// text field, with its `beforeinput`/`input`, and no keydown (Chrome
    /// fires none for a menu click). Whether a text field took it; false
    /// with none focused.
    pub fn run_edit_command(&mut self, id: EngineViewId, command: &EditCommand) -> bool {
        let _hold = wgate::hold("key");
        let Some(focused) = self.views.get(&id).and_then(|v| v.lay.focused_node.get()).map(|n| n.raw()) else {
            return false;
        };
        if !self.focused_is_text_control(id) {
            return false;
        }
        if self.is_editing_host(id, focused) {
            let letter = match command {
                EditCommand::Copy => "c",
                EditCommand::Cut => "x",
                EditCommand::Paste => "v",
                EditCommand::SelectAll => "a",
                EditCommand::Undo => "z",
                _ => return false,
            };
            return self.editable_key(id, focused, 0, letter, true);
        }
        self.user_edit_command(id, focused, command);
        true
    }
}

/// Where the renderer draws a text control's caret (`draw_text_input`):
/// after the measured width of the first `pos` characters of the caret's
/// line, from the content edge, one line of text high. The line height is
/// the font size times 1.2, the renderer's seat; close enough for an IME's
/// candidate window, which is all this is for.
///
/// Best effort (#918 R1 nit 8): it measures the raw value, not a password
/// field's bullets, and ignores the field's own horizontal or `<textarea>`
/// scroll, `text-align` and page zoom, so the window can sit off the caret
/// in those fields.
#[allow(clippy::too_many_arguments)]
fn control_caret(
    rect: rustkit_layout::Rect,
    value: &str,
    pos: usize,
    font_size: f32,
    border_width: f32,
    padding: [f32; 4],
    font_family: &str,
    font_weight: u16,
) -> TextInputCaret {
    let prefix: String = value.chars().take(pos).collect();
    let line = prefix.matches('\n').count() as f32;
    let on_line = prefix.rsplit('\n').next().unwrap_or("");
    let width = rustkit_layout::measure_text_advanced(
        on_line,
        font_family,
        font_size,
        rustkit_css::FontWeight(font_weight),
        rustkit_css::FontStyle::Normal,
    )
    .width;
    let height = font_size * 1.2;
    TextInputCaret {
        x: rect.x + border_width + padding[3] + width,
        y: rect.y + border_width + padding[0] + line * height,
        height,
    }
}

/// The system clipboard as the engine's [`rustkit_dom::clipboard::Clipboard`]:
/// macOS `NSPasteboard`'s general pasteboard, plain text (CH2.E3). On
/// another OS, an in-memory clipboard.
pub(crate) fn system_clipboard() -> rustkit_dom::clipboard::SharedClipboard {
    #[cfg(target_os = "macos")]
    {
        std::rc::Rc::new(Pasteboard)
    }
    #[cfg(not(target_os = "macos"))]
    {
        rustkit_dom::clipboard::MemoryClipboard::shared()
    }
}

/// The general pasteboard's plain text. `NSPasteboard` may be used from
/// any thread; the page's engine runs on the content thread.
#[cfg(target_os = "macos")]
struct Pasteboard;

#[cfg(target_os = "macos")]
impl Pasteboard {
    /// `NSPasteboardTypeString`.
    fn string_type() -> cocoa::base::id {
        use cocoa::foundation::NSString;
        use objc::{msg_send, sel, sel_impl};
        unsafe {
            let s = NSString::alloc(cocoa::base::nil).init_str("public.utf8-plain-text");
            msg_send![s, autorelease]
        }
    }
}

#[cfg(target_os = "macos")]
impl rustkit_dom::clipboard::Clipboard for Pasteboard {
    fn read_text(&self) -> Option<String> {
        use cocoa::base::{id, nil};
        use objc::{class, msg_send, sel, sel_impl};
        unsafe {
            let pool: id = msg_send![class!(NSAutoreleasePool), new];
            let board: id = msg_send![class!(NSPasteboard), generalPasteboard];
            let string: id = msg_send![board, stringForType: Self::string_type()];
            let text = if string == nil {
                None
            } else {
                let utf8: *const std::os::raw::c_char = msg_send![string, UTF8String];
                (!utf8.is_null()).then(|| std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
            };
            let _: () = msg_send![pool, drain];
            text
        }
    }

    fn write_text(&self, text: &str) {
        use cocoa::base::id;
        use cocoa::foundation::NSString;
        use objc::{class, msg_send, sel, sel_impl};
        unsafe {
            let pool: id = msg_send![class!(NSAutoreleasePool), new];
            let board: id = msg_send![class!(NSPasteboard), generalPasteboard];
            let _: isize = msg_send![board, clearContents];
            let string: id = NSString::alloc(cocoa::base::nil).init_str(text);
            let string: id = msg_send![string, autorelease];
            let _: bool = msg_send![board, setString: string forType: Self::string_type()];
            let _: () = msg_send![pool, drain];
        }
    }
}
