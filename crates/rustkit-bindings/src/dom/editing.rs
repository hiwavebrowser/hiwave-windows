//! Edit commands for script and for the user's shortcuts (CH2.E1):
//! `document.execCommand`/`queryCommandSupported`, `navigator.clipboard`,
//! and [`crate::DomBindings::user_edit`], which fires `beforeinput` and
//! `input` around a command the engine's key mapping chose.
//!
//! Every command runs on rustkit-dom's `TextEditState` (the same model the
//! engine's keyboard path drives): the engine shares its per-view map of
//! edit states (`DomBindings::share_edit_states`), so a script's
//! `execCommand('undo')` undoes what the user typed. Without an engine the
//! bindings keep a map of their own.

use rustkit_dom::clipboard::{MemoryClipboard, SharedClipboard};
use rustkit_dom::forms::TextEditState;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// A view's text-control edit states, by raw NodeId.
pub type SharedEditStates = Rc<RefCell<HashMap<usize, TextEditState>>>;

/// What the edit host functions share with `DomBindings`.
pub struct EditContext {
    pub(crate) clipboard: RefCell<SharedClipboard>,
    pub(crate) states: RefCell<SharedEditStates>,
    /// Whether script may read the clipboard (`readText`,
    /// `execCommand('paste')`). Off by default: the clipboard is shared by
    /// every view, so a web page reading it could see what the user copied
    /// in another tab. Trusted views (HiWave's own chrome) opt in. A
    /// user's paste is always allowed.
    pub(crate) script_read: Cell<bool>,
    /// Until when the page has transient user activation (HTML §6.4.1): a
    /// trusted key press or mouse press grants it for
    /// [`ACTIVATION_DURATION`]. `navigator.clipboard.writeText` needs it,
    /// so a page cannot overwrite the clipboard on its own.
    pub(crate) activation_until: Cell<Option<std::time::Instant>>,
    /// Bumped each time script sets a text control's selection
    /// (`select()`, `setSelectionRange`, the `selectionStart`/`End`
    /// setters). The engine reads it around a press's focus change: a
    /// focus handler that selected keeps its selection (CH2.E12).
    pub(crate) script_selections: Cell<u64>,
}

/// How long a user activation lasts (Chrome's transient activation
/// duration).
pub(crate) const ACTIVATION_DURATION: std::time::Duration = std::time::Duration::from_secs(5);

impl EditContext {
    pub(crate) fn activate(&self) {
        self.activation_until
            .set(Some(std::time::Instant::now() + ACTIVATION_DURATION));
    }

    pub(crate) fn has_activation(&self) -> bool {
        self.activation_until
            .get()
            .is_some_and(|until| std::time::Instant::now() < until)
    }
}

impl Default for EditContext {
    fn default() -> Self {
        Self {
            clipboard: RefCell::new(MemoryClipboard::shared()),
            states: RefCell::new(Rc::default()),
            script_read: Cell::new(false),
            activation_until: Cell::new(None),
            script_selections: Cell::new(0),
        }
    }
}

use super::{control_record, string_arg, SharedDomHost};
use crate::{DirtyMark, DomDirty, MutationKind, MutationLog, ScriptMutations};
use rustkit_dom::forms::{text_control_kind, EditCommand, TextControlKind};
use rustkit_js::{JsError, JsRuntime, JsValue};

/// UTF-16 offset `units` of `s` as a byte offset (clamped, on a boundary).
fn byte_offset(s: &str, units: usize) -> usize {
    let mut used = 0;
    for (i, c) in s.char_indices() {
        if used >= units {
            return i;
        }
        used += c.len_utf16();
    }
    s.len()
}

/// Byte offset `bytes` of `s` in UTF-16 code units, script's unit.
fn utf16_offset(s: &str, bytes: usize) -> usize {
    s[..bytes.min(s.len())].chars().map(char::len_utf16).sum()
}

fn number_arg(args: &[JsValue], index: usize) -> Option<usize> {
    match args.get(index) {
        Some(JsValue::Number(n)) if *n >= 0.0 => Some(*n as usize),
        _ => None,
    }
}

/// An edit command by name: `execCommand`'s names plus the user-only
/// word deletions.
fn command(name: &str, value: Option<&str>) -> Option<EditCommand> {
    match name {
        "deleteWordBackward" => Some(EditCommand::DeleteWordBackward),
        "deleteWordForward" => Some(EditCommand::DeleteWordForward),
        _ => EditCommand::from_exec_name(name, value),
    }
}

pub(crate) fn install(
    runtime: &mut JsRuntime,
    host: &SharedDomHost,
    dirty: &Rc<DirtyMark>,
    mutations: &Rc<RefCell<ScriptMutations>>,
    log: &Rc<RefCell<MutationLog>>,
    ctx: &Rc<EditContext>,
) -> Result<(), JsError> {
    // clip_read(user): the clipboard's text ('' when it holds none), or
    // false when script may not read it (`user` false and reads denied).
    let c = ctx.clone();
    runtime.register_host_function(
        "__rustkit_clip_read",
        1,
        Box::new(move |args| {
            let user = matches!(args.first(), Some(JsValue::Boolean(true)));
            if !user && !c.script_read.get() {
                return JsValue::Boolean(false);
            }
            let clipboard = c.clipboard.borrow().clone();
            JsValue::String(clipboard.read_text().unwrap_or_default())
        }),
    )?;

    // clip_write(text): put `text` on the clipboard; false (nothing
    // written) without a user activation.
    let c = ctx.clone();
    runtime.register_host_function(
        "__rustkit_clip_write",
        1,
        Box::new(move |args| {
            if !c.has_activation() {
                return JsValue::Boolean(false);
            }
            let clipboard = c.clipboard.borrow().clone();
            clipboard.write_text(string_arg(args, 0).unwrap_or(""));
            JsValue::Boolean(true)
        }),
    )?;

    // edit(gen, id, phase, cmd, value, start, end, user): run edit command
    // `cmd` (`value` for insertText) on text control `id`. `start`/`end`
    // (UTF-16) set the selection first; null keeps the edit state's (the
    // user's caret). Phase "check" answers whether it is enabled; "run"
    // answers a JSON report (handled, changed, start, end, dir,
    // inputType, data). Null when `id` is not a text control.
    let h = host.clone();
    let d = dirty.clone();
    let m = mutations.clone();
    let l = log.clone();
    let c = ctx.clone();
    runtime.register_host_function(
        "__rustkit_edit",
        8,
        Box::new(move |args| {
            let Some(node) = h.borrow().node(args) else {
                return JsValue::Null;
            };
            let Some(kind) = text_control_kind(&node) else {
                return JsValue::Null;
            };
            let Some(cmd) = string_arg(args, 3).and_then(|n| command(n, string_arg(args, 4)))
            else {
                return JsValue::Null;
            };
            let user = matches!(args.get(7), Some(JsValue::Boolean(true)));
            if cmd == EditCommand::Paste && !user && !c.script_read.get() {
                return match string_arg(args, 2) {
                    Some("check") => JsValue::Boolean(false),
                    _ => JsValue::Null,
                };
            }
            let raw = node.id.raw();
            let current = h.borrow().values.get(&raw).cloned().unwrap_or_else(|| {
                if kind == TextControlKind::MultiLine {
                    node.text_content()
                } else {
                    node.get_attribute("value").unwrap_or("").to_string()
                }
            });
            let states = c.states.borrow().clone();
            let mut states = states.borrow_mut();
            let state = states.entry(raw).or_insert_with(|| {
                let s = TextEditState::with_value(current.clone());
                s.move_to_end(false);
                s
            });
            if state.value() != current {
                // Script set the value since: a new, unhistoried text.
                state.set_value(current);
            }
            state.sync_attributes(&node);
            if let (Some(start), Some(end)) = (number_arg(args, 5), number_arg(args, 6)) {
                let value = state.value();
                let (start, end) = (byte_offset(&value, start), byte_offset(&value, end));
                state.set_selection(start.min(end), end);
            }
            let clipboard = c.clipboard.borrow().clone();
            if string_arg(args, 2) == Some("check") {
                return JsValue::Boolean(state.can_run(&cmd, &*clipboard));
            }
            let out = state.run_command(&cmd, &*clipboard);
            let value = state.value();
            let sel = state.selection().normalize();
            let dir = match state.selection_direction() {
                rustkit_dom::SelectionDirection::Forward => "forward",
                rustkit_dom::SelectionDirection::Backward => "backward",
                rustkit_dom::SelectionDirection::None => "none",
            };
            let report = serde_json::json!({
                "handled": out.handled,
                "changed": out.changed,
                "start": utf16_offset(&value, sel.start),
                "end": utf16_offset(&value, sel.end),
                "dir": dir,
                "inputType": out.input_type,
                "data": out.data,
            });
            drop(states);
            if out.changed {
                // Script reads the new value; the engine paints it from the
                // (shared) edit state, so no value write is queued: one
                // would reset the state and drop its undo history.
                h.borrow_mut().values.insert(raw, value);
                l.borrow_mut().push(control_record(raw, None));
                super::note_mutation(&m, DomDirty::Layout, MutationKind::FormValue, || {
                    format!("{}#edit", node.tag_name().unwrap_or(""))
                });
                d.mark(DomDirty::Layout, "form");
            }
            JsValue::String(report.to_string())
        }),
    )?;
    // text_sel(gen, id, start?, end?, dir?): a text control's selection in
    // the edit state the engine edits and paints from (CH2.E12), as JSON
    // {start, end, dir} in UTF-16 units. With `start` and `end` it is set
    // first (the state is made when the control has none yet); without,
    // null when the control has no state (script keeps its own copy).
    // Null when `id` is not a text control.
    let h = host.clone();
    let d = dirty.clone();
    let c = ctx.clone();
    runtime.register_host_function(
        "__rustkit_text_sel",
        5,
        Box::new(move |args| {
            let Some(node) = h.borrow().node(args) else {
                return JsValue::Null;
            };
            let Some(kind) = text_control_kind(&node) else {
                return JsValue::Null;
            };
            let raw = node.id.raw();
            let set = match (number_arg(args, 2), number_arg(args, 3)) {
                (Some(start), Some(end)) => Some((start, end)),
                _ => None,
            };
            let current = h.borrow().values.get(&raw).cloned().unwrap_or_else(|| {
                if kind == TextControlKind::MultiLine {
                    node.text_content()
                } else {
                    node.get_attribute("value").unwrap_or("").to_string()
                }
            });
            let states = c.states.borrow().clone();
            let mut states = states.borrow_mut();
            if set.is_none() && !states.contains_key(&raw) {
                return JsValue::Null;
            }
            let state = states.entry(raw).or_insert_with(|| {
                let s = TextEditState::with_value(current.clone());
                s.move_to_end(false);
                s
            });
            if state.value() != current {
                // Script set the value since: the caret goes to its end.
                state.set_value(current);
            }
            let mut moved = false;
            if let Some((start, end)) = set {
                let value = state.value();
                let (start, end) = (byte_offset(&value, start), byte_offset(&value, end));
                let dir = match string_arg(args, 4) {
                    Some("forward") => rustkit_dom::SelectionDirection::Forward,
                    Some("backward") => rustkit_dom::SelectionDirection::Backward,
                    _ => rustkit_dom::SelectionDirection::None,
                };
                let before = (state.selection().normalize(), state.selection_direction());
                state.set_selection_with_direction(start.min(end), end, dir);
                moved = before != (state.selection().normalize(), state.selection_direction());
                c.script_selections.set(c.script_selections.get() + 1);
            }
            let value = state.value();
            let sel = state.selection().normalize();
            let dir = match state.selection_direction() {
                rustkit_dom::SelectionDirection::Forward => "forward",
                rustkit_dom::SelectionDirection::Backward => "backward",
                rustkit_dom::SelectionDirection::None => "none",
            };
            let report = serde_json::json!({
                "start": utf16_offset(&value, sel.start),
                "end": utf16_offset(&value, sel.end),
                "dir": dir,
            });
            drop(states);
            if moved {
                // The engine paints a focused control's selection from the
                // state: lay it out again.
                d.mark(DomDirty::Layout, "form");
            }
            JsValue::String(report.to_string())
        }),
    )?;
    runtime.evaluate_script(include_str!("editing.js"))?;
    Ok(())
}
