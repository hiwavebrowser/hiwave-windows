//! # Form Elements
//!
//! Implementation of HTML form elements including input, textarea, button, and select.
//! Provides text editing, selection, and form submission support.

use std::cell::{Cell, RefCell};

/// Text selection range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SelectionRange {
    /// Start of selection (0-indexed character position).
    pub start: usize,
    /// End of selection (0-indexed character position).
    pub end: usize,
}

impl SelectionRange {
    /// Create a new selection range.
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Create a collapsed selection (caret) at a position.
    pub fn caret(position: usize) -> Self {
        Self {
            start: position,
            end: position,
        }
    }

    /// Check if the selection is collapsed (just a caret).
    pub fn is_collapsed(&self) -> bool {
        self.start == self.end
    }

    /// Get the length of the selection.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Check if the selection is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Normalize the selection so start <= end.
    pub fn normalize(&self) -> Self {
        if self.start <= self.end {
            *self
        } else {
            Self {
                start: self.end,
                end: self.start,
            }
        }
    }

    /// Expand selection to include a position.
    pub fn extend_to(&self, position: usize) -> Self {
        if position < self.start {
            Self {
                start: position,
                end: self.end,
            }
        } else {
            Self {
                start: self.start,
                end: position,
            }
        }
    }
}

/// Selection direction for Shift+Arrow navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionDirection {
    #[default]
    Forward,
    Backward,
    None,
}

/// Input type for HTML input elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputType {
    #[default]
    Text,
    Password,
    Email,
    Url,
    Tel,
    Number,
    Search,
    Hidden,
    Submit,
    Button,
    Reset,
    Checkbox,
    Radio,
    File,
    Image,
    Color,
    Date,
    DatetimeLocal,
    Month,
    Week,
    Time,
    Range,
}

impl InputType {
    /// Parse from a string (case-insensitive).
    /// Parse input type from string.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "text" => InputType::Text,
            "password" => InputType::Password,
            "email" => InputType::Email,
            "url" => InputType::Url,
            "tel" => InputType::Tel,
            "number" => InputType::Number,
            "search" => InputType::Search,
            "hidden" => InputType::Hidden,
            "submit" => InputType::Submit,
            "button" => InputType::Button,
            "reset" => InputType::Reset,
            "checkbox" => InputType::Checkbox,
            "radio" => InputType::Radio,
            "file" => InputType::File,
            "image" => InputType::Image,
            "color" => InputType::Color,
            "date" => InputType::Date,
            "datetime-local" => InputType::DatetimeLocal,
            "month" => InputType::Month,
            "week" => InputType::Week,
            "time" => InputType::Time,
            "range" => InputType::Range,
            _ => InputType::Text,
        }
    }

    /// Check if this input type accepts text.
    pub fn is_text_input(&self) -> bool {
        matches!(
            self,
            InputType::Text
                | InputType::Password
                | InputType::Email
                | InputType::Url
                | InputType::Tel
                | InputType::Number
                | InputType::Search
        )
    }

    /// Check if this is a button type.
    pub fn is_button(&self) -> bool {
        matches!(
            self,
            InputType::Submit | InputType::Button | InputType::Reset | InputType::Image
        )
    }

    /// Check if this is a checkable input.
    pub fn is_checkable(&self) -> bool {
        matches!(self, InputType::Checkbox | InputType::Radio)
    }
}

/// Text editing state for input/textarea elements.
///
/// Offsets are byte offsets into the value, always on char boundaries.
/// `maxlength` counts UTF-16 code units, as HTML does.
#[derive(Debug)]
#[allow(dead_code)]
pub struct TextEditState {
    /// The text value.
    value: RefCell<String>,
    /// Current selection.
    selection: Cell<SelectionRange>,
    /// Selection direction: which end is the focus (the end that moves
    /// when Shift+arrow extends). `Backward` puts the focus at `start`.
    selection_direction: Cell<SelectionDirection>,
    /// Maximum length (None = unlimited).
    max_length: Cell<Option<usize>>,
    /// Minimum length for validation.
    min_length: Cell<usize>,
    /// Whether the field is read-only.
    read_only: Cell<bool>,
    /// Whether the field is disabled.
    disabled: Cell<bool>,
    /// Placeholder text.
    placeholder: RefCell<String>,
    /// Pattern for validation (regex string).
    pattern: RefCell<Option<String>>,
    /// Whether required for form submission.
    required: Cell<bool>,
    /// What kind of control this edits (paste and copy rules).
    kind: Cell<TextControlKind>,
    /// Undo/redo history.
    history: RefCell<EditHistory>,
}

/// The value and selection before or after an edit.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EditSnapshot {
    value: String,
    selection: SelectionRange,
    direction: SelectionDirection,
}

/// How an edit joins the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    /// A typed character: joins the open typing entry.
    Typing,
    /// A paste, cut, delete or script insertion: its own entry.
    Discrete,
}

/// Bounded undo/redo stacks of snapshots taken before each entry.
#[derive(Debug, Default)]
struct EditHistory {
    undo: std::collections::VecDeque<EditSnapshot>,
    redo: Vec<EditSnapshot>,
    /// The top undo entry is typing that the next typed character joins.
    /// Any caret movement or other edit closes it.
    typing_open: bool,
}

impl EditHistory {
    fn record(&mut self, before: EditSnapshot, kind: EditKind) {
        self.redo.clear();
        if kind == EditKind::Typing && self.typing_open && !self.undo.is_empty() {
            return;
        }
        self.undo.push_back(before);
        while self.undo.len() > UNDO_LIMIT {
            self.undo.pop_front();
        }
        self.typing_open = kind == EditKind::Typing;
    }

    fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.typing_open = false;
    }
}

impl Default for TextEditState {
    fn default() -> Self {
        Self::new()
    }
}

/// The char boundary before `pos` in `s` (0 at the start).
fn prev_boundary(s: &str, pos: usize) -> usize {
    s[..pos.min(s.len())]
        .char_indices()
        .next_back()
        .map_or(0, |(i, _)| i)
}

/// The char boundary after `pos` in `s` (`s.len()` at the end).
fn next_boundary(s: &str, pos: usize) -> usize {
    let pos = pos.min(s.len());
    s[pos..].chars().next().map_or(pos, |c| pos + c.len_utf8())
}

/// `pos` moved back onto a char boundary of `s`.
fn floor_boundary(s: &str, pos: usize) -> usize {
    let mut pos = pos.min(s.len());
    while !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// Characters that make up a word for word movement and deletion.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The start of the word at or before `pos`: skip non-word chars back,
/// then word chars.
fn word_start_before(s: &str, pos: usize) -> usize {
    let mut i = pos.min(s.len());
    let before = |i: usize| s[..i].chars().next_back();
    while before(i).is_some_and(|c| !is_word_char(c)) {
        i = prev_boundary(s, i);
    }
    while before(i).is_some_and(is_word_char) {
        i = prev_boundary(s, i);
    }
    i
}

/// The end of the word at or after `pos`: skip non-word chars, then word
/// chars (macOS Option+Right, and delete-word-forward).
fn word_end_after(s: &str, pos: usize) -> usize {
    let mut i = pos.min(s.len());
    let at = |i: usize| s[i..].chars().next();
    while at(i).is_some_and(|c| !is_word_char(c)) {
        i = next_boundary(s, i);
    }
    while at(i).is_some_and(is_word_char) {
        i = next_boundary(s, i);
    }
    i
}

/// The start of the next word after `pos`: skip word chars, then the
/// rest (Windows/Linux Ctrl+Right).
fn next_word_start(s: &str, pos: usize) -> usize {
    let mut i = pos.min(s.len());
    let at = |i: usize| s[i..].chars().next();
    while at(i).is_some_and(is_word_char) {
        i = next_boundary(s, i);
    }
    while at(i).is_some_and(|c| !is_word_char(c)) {
        i = next_boundary(s, i);
    }
    i
}

/// Length in UTF-16 code units, the unit of `maxlength`.
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// The longest prefix of `s` at most `units` UTF-16 code units long.
fn utf16_prefix(s: &str, units: usize) -> &str {
    let mut used = 0;
    for (i, c) in s.char_indices() {
        used += c.len_utf16();
        if used > units {
            return &s[..i];
        }
    }
    s
}

impl TextEditState {
    /// Create a new empty text edit state.
    pub fn new() -> Self {
        Self::with_value(String::new())
    }

    /// Create with an initial value.
    pub fn with_value(value: impl Into<String>) -> Self {
        let text = value.into();
        let len = text.len();
        Self {
            value: RefCell::new(text),
            selection: Cell::new(SelectionRange::caret(len)),
            selection_direction: Cell::new(SelectionDirection::None),
            max_length: Cell::new(None),
            min_length: Cell::new(0),
            read_only: Cell::new(false),
            disabled: Cell::new(false),
            placeholder: RefCell::new(String::new()),
            pattern: RefCell::new(None),
            required: Cell::new(false),
            kind: Cell::new(TextControlKind::SingleLine),
            history: RefCell::new(EditHistory::default()),
        }
    }

    /// Get the current value.
    pub fn value(&self) -> String {
        self.value.borrow().clone()
    }

    /// Set the value (resets selection to end). A value set this way (by
    /// script, or a form reset) is not undoable and clears the history.
    pub fn set_value(&self, value: impl Into<String>) {
        let text = value.into();
        let len = text.len();
        *self.value.borrow_mut() = text;
        self.selection.set(SelectionRange::caret(len));
        self.selection_direction.set(SelectionDirection::None);
        self.history.borrow_mut().clear();
    }

    /// Get the current selection.
    pub fn selection(&self) -> SelectionRange {
        self.selection.get()
    }

    /// The selection's direction (which end Shift+arrow moves).
    pub fn selection_direction(&self) -> SelectionDirection {
        self.selection_direction.get()
    }

    /// Set the selection range.
    pub fn set_selection(&self, start: usize, end: usize) {
        let value = self.value.borrow();
        let start = floor_boundary(&value, start);
        let end = floor_boundary(&value, end);
        drop(value);
        self.selection.set(SelectionRange::new(start, end));
        self.selection_direction.set(SelectionDirection::None);
        self.close_typing();
    }

    /// Set the selection range and its direction, as script's
    /// `setSelectionRange(start, end, direction)` does.
    pub fn set_selection_with_direction(&self, start: usize, end: usize, direction: SelectionDirection) {
        self.set_selection(start, end);
        self.selection_direction.set(direction);
    }

    /// Get the caret position (selection start).
    pub fn caret_position(&self) -> usize {
        self.selection.get().start
    }

    /// The selection's focus: the end that moves when Shift+arrow extends
    /// (the caret when collapsed).
    pub fn focus_position(&self) -> usize {
        self.anchor_focus().1
    }

    /// Set the caret position.
    pub fn set_caret(&self, position: usize) {
        self.place(position, position);
    }

    /// Get selected text.
    pub fn selected_text(&self) -> String {
        let sel = self.selection.get().normalize();
        let value = self.value.borrow();
        if sel.start <= value.len() && sel.end <= value.len() {
            value[sel.start..sel.end].to_string()
        } else {
            String::new()
        }
    }

    /// Select all text.
    pub fn select_all(&self) {
        let len = self.value.borrow().len();
        self.place(0, len);
    }

    /// Check if the input can be edited.
    pub fn is_editable(&self) -> bool {
        !self.read_only.get() && !self.disabled.get()
    }

    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            value: self.value(),
            selection: self.selection.get(),
            direction: self.selection_direction.get(),
        }
    }

    fn restore(&self, snap: EditSnapshot) {
        *self.value.borrow_mut() = snap.value;
        self.selection.set(snap.selection);
        self.selection_direction.set(snap.direction);
    }

    fn close_typing(&self) {
        self.history.borrow_mut().typing_open = false;
    }

    /// The selection's (anchor, focus).
    fn anchor_focus(&self) -> (usize, usize) {
        let sel = self.selection.get().normalize();
        match self.selection_direction.get() {
            SelectionDirection::Backward => (sel.end, sel.start),
            _ => (sel.start, sel.end),
        }
    }

    /// Select from `anchor` to `focus` (a caret when equal).
    fn place(&self, anchor: usize, focus: usize) {
        let value = self.value.borrow();
        let anchor = floor_boundary(&value, anchor);
        let focus = floor_boundary(&value, focus);
        drop(value);
        let (sel, dir) = if focus < anchor {
            (
                SelectionRange::new(focus, anchor),
                SelectionDirection::Backward,
            )
        } else if focus > anchor {
            (
                SelectionRange::new(anchor, focus),
                SelectionDirection::Forward,
            )
        } else {
            (SelectionRange::caret(focus), SelectionDirection::None)
        };
        self.selection.set(sel);
        self.selection_direction.set(dir);
        self.close_typing();
    }

    /// Move the caret to `pos`, or with `extend` move the selection's focus
    /// there and keep its anchor.
    pub fn move_caret_to(&self, pos: usize, extend: bool) {
        if extend {
            let (anchor, _) = self.anchor_focus();
            self.place(anchor, pos);
        } else {
            self.place(pos, pos);
        }
    }

    /// Replace the selection with `text` (already sanitized and fitted),
    /// recording the edit. The caret ends after the text.
    fn replace_selection(&self, text: &str, kind: EditKind) {
        let before = self.snapshot();
        let sel = self.selection.get().normalize();
        let mut value = self.value.borrow_mut();
        let start = floor_boundary(&value, sel.start);
        let end = floor_boundary(&value, sel.end);
        value.replace_range(start..end, text);
        drop(value);
        self.selection
            .set(SelectionRange::caret(start + text.len()));
        self.selection_direction.set(SelectionDirection::None);
        self.history.borrow_mut().record(before, kind);
    }

    /// Delete `start..end` as one undoable edit; the caret ends at `start`.
    fn delete_range(&self, start: usize, end: usize) -> bool {
        if start >= end {
            return false;
        }
        let before = self.snapshot();
        self.value.borrow_mut().replace_range(start..end, "");
        self.selection.set(SelectionRange::caret(start));
        self.selection_direction.set(SelectionDirection::None);
        self.history.borrow_mut().record(before, EditKind::Discrete);
        true
    }

    /// Room left under `maxlength` once the selection is replaced, in
    /// UTF-16 code units (`None`: unlimited).
    fn room(&self) -> Option<usize> {
        let max = self.max_length.get()?;
        let sel = self.selection.get().normalize();
        let value = self.value.borrow();
        let kept = utf16_len(&value)
            - utf16_len(&value[sel.start.min(value.len())..sel.end.min(value.len())]);
        Some(max.saturating_sub(kept))
    }

    /// Insert text at the current caret position (or replace selection),
    /// as typing: consecutive typed characters undo together. Refused
    /// (false) when it would pass `maxlength`.
    pub fn insert_text(&self, text: &str) -> bool {
        if !self.is_editable() {
            return false;
        }
        if self.room().is_some_and(|room| utf16_len(text) > room) {
            return false;
        }
        self.replace_selection(text, EditKind::Typing);
        true
    }

    /// Delete the selection or character before caret (backspace).
    pub fn delete_backward(&self) -> bool {
        if !self.is_editable() {
            return false;
        }
        let sel = self.selection.get().normalize();
        if !sel.is_collapsed() {
            return self.delete_range(sel.start, sel.end);
        }
        let start = prev_boundary(&self.value.borrow(), sel.start);
        self.delete_range(start, sel.start)
    }

    /// Delete the selection or character after caret (delete key).
    pub fn delete_forward(&self) -> bool {
        if !self.is_editable() {
            return false;
        }
        let sel = self.selection.get().normalize();
        if !sel.is_collapsed() {
            return self.delete_range(sel.start, sel.end);
        }
        let end = next_boundary(&self.value.borrow(), sel.start);
        self.delete_range(sel.start, end)
    }

    /// Move caret left (by one character); with `extend_selection`, move
    /// the selection's focus.
    pub fn move_left(&self, extend_selection: bool) {
        let sel = self.selection.get().normalize();
        if !extend_selection && !sel.is_collapsed() {
            // Collapse to start
            self.place(sel.start, sel.start);
            return;
        }
        let (_, focus) = self.anchor_focus();
        let pos = prev_boundary(&self.value.borrow(), focus);
        self.move_caret_to(pos, extend_selection);
    }

    /// Move caret right (by one character).
    pub fn move_right(&self, extend_selection: bool) {
        let sel = self.selection.get().normalize();
        if !extend_selection && !sel.is_collapsed() {
            // Collapse to end
            self.place(sel.end, sel.end);
            return;
        }
        let (_, focus) = self.anchor_focus();
        let pos = next_boundary(&self.value.borrow(), focus);
        self.move_caret_to(pos, extend_selection);
    }

    /// Move caret to start.
    pub fn move_to_start(&self, extend_selection: bool) {
        self.move_caret_to(0, extend_selection);
    }

    /// Move caret to end.
    pub fn move_to_end(&self, extend_selection: bool) {
        let len = self.value.borrow().len();
        self.move_caret_to(len, extend_selection);
    }

    /// Get/set properties.
    pub fn is_read_only(&self) -> bool {
        self.read_only.get()
    }

    pub fn set_read_only(&self, read_only: bool) {
        self.read_only.set(read_only);
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled.get()
    }

    pub fn set_disabled(&self, disabled: bool) {
        self.disabled.set(disabled);
    }

    pub fn placeholder(&self) -> String {
        self.placeholder.borrow().clone()
    }

    pub fn set_placeholder(&self, placeholder: impl Into<String>) {
        *self.placeholder.borrow_mut() = placeholder.into();
    }

    pub fn max_length(&self) -> Option<usize> {
        self.max_length.get()
    }

    pub fn set_max_length(&self, max: Option<usize>) {
        self.max_length.set(max);
    }

    pub fn min_length(&self) -> usize {
        self.min_length.get()
    }

    pub fn set_min_length(&self, min: usize) {
        self.min_length.set(min);
    }

    pub fn is_required(&self) -> bool {
        self.required.get()
    }

    pub fn set_required(&self, required: bool) {
        self.required.set(required);
    }

    /// Basic validation check.
    pub fn is_valid(&self) -> bool {
        let value = self.value.borrow();
        let len = value.len();

        // Required check
        if self.required.get() && len == 0 {
            return false;
        }

        // Min length check
        if len < self.min_length.get() && len > 0 {
            return false;
        }

        // Max length check
        if let Some(max) = self.max_length.get() {
            if len > max {
                return false;
            }
        }

        true
    }
}

/// How many edits a text control can undo.
pub const UNDO_LIMIT: usize = 100;

/// What kind of text control a [`TextEditState`] edits: decides what a
/// paste may insert and whether the text may leave the control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextControlKind {
    /// A single-line `<input>` (text, search, url, tel, email, ...): a
    /// paste drops line breaks (the value sanitization algorithm).
    #[default]
    SingleLine,
    /// `<input type=password>`: single-line, and copy/cut are refused.
    Password,
    /// `<textarea>`: line breaks are kept (normalized to LF).
    MultiLine,
}

/// Which platform's editing key conventions apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPlatform {
    /// macOS: Cmd is the command key, Option+arrow moves by word,
    /// Cmd+arrow moves to the line's ends.
    Mac,
    /// Windows/Linux: Ctrl is the command key and Ctrl+arrow moves by word.
    Other,
}

impl KeyPlatform {
    /// The platform this build runs on.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            KeyPlatform::Mac
        } else {
            KeyPlatform::Other
        }
    }
}

/// An edit command on a text control: what a shortcut,
/// `document.execCommand` or the engine asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditCommand {
    Copy,
    Cut,
    Paste,
    SelectAll,
    Undo,
    Redo,
    /// Insert (or replace the selection with) this text.
    InsertText(String),
    DeleteWordBackward,
    DeleteWordForward,
}

impl EditCommand {
    /// The `document.execCommand` name's command, if supported here.
    /// Names match ASCII case-insensitively; `insertText` takes `value`.
    pub fn from_exec_name(name: &str, value: Option<&str>) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "copy" => EditCommand::Copy,
            "cut" => EditCommand::Cut,
            "paste" => EditCommand::Paste,
            "selectall" => EditCommand::SelectAll,
            "undo" => EditCommand::Undo,
            "redo" => EditCommand::Redo,
            "inserttext" => EditCommand::InsertText(value.unwrap_or("").to_string()),
            "forwarddelete" => EditCommand::DeleteWordForward,
            _ => return None,
        })
    }

    /// The Input Events `inputType` of the edit this command makes, `None`
    /// for one that never changes the value (copy, select-all).
    pub fn input_type(&self) -> Option<&'static str> {
        Some(match self {
            EditCommand::Copy | EditCommand::SelectAll => return None,
            EditCommand::Cut => "deleteByCut",
            EditCommand::Paste => "insertFromPaste",
            EditCommand::Undo => "historyUndo",
            EditCommand::Redo => "historyRedo",
            EditCommand::InsertText(_) => "insertText",
            EditCommand::DeleteWordBackward => "deleteWordBackward",
            EditCommand::DeleteWordForward => "deleteWordForward",
        })
    }

    /// Whether the command can change the control's value.
    pub fn mutates(&self) -> bool {
        self.input_type().is_some()
    }
}

/// What running an [`EditCommand`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditOutcome {
    /// The command applied to this control (it was enabled).
    pub handled: bool,
    /// The value changed.
    pub changed: bool,
    /// The selection changed (and the value did not).
    pub selection_changed: bool,
    /// The `inputType` of the change, when `changed`.
    pub input_type: Option<&'static str>,
    /// The `data` of the change's input event (inserted text), when any.
    pub data: Option<String>,
}

impl TextEditState {
    /// What kind of control this edits.
    pub fn kind(&self) -> TextControlKind {
        self.kind.get()
    }

    /// Set what kind of control this edits (`<input type>`/`<textarea>`).
    pub fn set_kind(&self, kind: TextControlKind) {
        self.kind.set(kind);
    }

    /// `text` as this control would hold it: a single-line control drops
    /// line breaks, a textarea normalizes them to LF (HTML value
    /// sanitization).
    pub fn sanitize(&self, text: &str) -> String {
        match self.kind.get() {
            TextControlKind::MultiLine => text.replace("\r\n", "\n").replace('\r', "\n"),
            _ => text.chars().filter(|c| !matches!(c, '\r' | '\n')).collect(),
        }
    }

    /// Whether the selection may be copied out (not from a password field).
    fn may_copy(&self) -> bool {
        self.kind.get() != TextControlKind::Password && !self.selection.get().is_collapsed()
    }

    /// Copy the selection to `clipboard`. False (clipboard untouched) for
    /// an empty selection or a password field.
    pub fn copy(&self, clipboard: &dyn crate::clipboard::Clipboard) -> bool {
        if !self.may_copy() {
            return false;
        }
        clipboard.write_text(&self.selected_text());
        true
    }

    /// Copy the selection to `clipboard` and delete it (one undo entry).
    pub fn cut(&self, clipboard: &dyn crate::clipboard::Clipboard) -> bool {
        if !self.is_editable() || !self.copy(clipboard) {
            return false;
        }
        let sel = self.selection.get().normalize();
        self.delete_range(sel.start, sel.end)
    }

    /// Paste the clipboard's text over the selection.
    pub fn paste(&self, clipboard: &dyn crate::clipboard::Clipboard) -> bool {
        match clipboard.read_text() {
            Some(text) => self.paste_text(&text),
            None => false,
        }
    }

    /// Paste `text` over the selection as one undo entry: sanitized for
    /// this control and truncated to fit `maxlength` (whole characters).
    /// False when nothing could be inserted.
    pub fn paste_text(&self, text: &str) -> bool {
        self.insert_discrete(text).is_some()
    }

    /// [`Self::paste_text`], answering the text actually inserted.
    fn insert_discrete(&self, text: &str) -> Option<String> {
        if !self.is_editable() {
            return None;
        }
        let clean = self.sanitize(text);
        let fitted = match self.room() {
            Some(room) => utf16_prefix(&clean, room).to_string(),
            None => clean,
        };
        if fitted.is_empty() && self.selection.get().is_collapsed() {
            return None;
        }
        if fitted.is_empty() && !text.is_empty() {
            // Nothing fits: browsers leave the selection in place.
            return None;
        }
        self.replace_selection(&fitted, EditKind::Discrete);
        Some(fitted)
    }

    /// Undo the last edit. False when there is none.
    pub fn undo(&self) -> bool {
        if !self.is_editable() {
            return false;
        }
        let Some(before) = self.history.borrow_mut().undo.pop_back() else {
            return false;
        };
        let now = self.snapshot();
        self.restore(before);
        let mut h = self.history.borrow_mut();
        h.redo.push(now);
        h.typing_open = false;
        true
    }

    /// Redo the last undone edit. False when there is none.
    pub fn redo(&self) -> bool {
        if !self.is_editable() {
            return false;
        }
        let Some(after) = self.history.borrow_mut().redo.pop() else {
            return false;
        };
        let now = self.snapshot();
        self.restore(after);
        let mut h = self.history.borrow_mut();
        h.undo.push_back(now);
        h.typing_open = false;
        true
    }

    /// Whether there is an edit to undo.
    pub fn can_undo(&self) -> bool {
        self.is_editable() && !self.history.borrow().undo.is_empty()
    }

    /// Whether there is an undone edit to redo.
    pub fn can_redo(&self) -> bool {
        self.is_editable() && !self.history.borrow().redo.is_empty()
    }

    /// Move the caret (or with `extend` the selection's focus) to the start
    /// of the word before it.
    pub fn move_word_left(&self, extend: bool) {
        let (_, focus) = self.anchor_focus();
        let pos = word_start_before(&self.value.borrow(), focus);
        self.move_caret_to(pos, extend);
    }

    /// Move the caret (or the focus) right by a word: to the end of the
    /// word on macOS, to the start of the next word elsewhere.
    pub fn move_word_right(&self, extend: bool, platform: KeyPlatform) {
        let (_, focus) = self.anchor_focus();
        let value = self.value.borrow();
        let pos = match platform {
            KeyPlatform::Mac => word_end_after(&value, focus),
            KeyPlatform::Other => next_word_start(&value, focus),
        };
        drop(value);
        self.move_caret_to(pos, extend);
    }

    /// Delete the selection, or back to the start of the word before the
    /// caret.
    pub fn delete_word_backward(&self) -> bool {
        if !self.is_editable() {
            return false;
        }
        let sel = self.selection.get().normalize();
        if !sel.is_collapsed() {
            return self.delete_range(sel.start, sel.end);
        }
        let start = word_start_before(&self.value.borrow(), sel.start);
        self.delete_range(start, sel.start)
    }

    /// Delete the selection, or up to the end of the word after the caret.
    pub fn delete_word_forward(&self) -> bool {
        if !self.is_editable() {
            return false;
        }
        let sel = self.selection.get().normalize();
        if !sel.is_collapsed() {
            return self.delete_range(sel.start, sel.end);
        }
        let end = word_end_after(&self.value.borrow(), sel.start);
        self.delete_range(sel.start, end)
    }

    /// Whether `cmd` is enabled here: it would do something.
    pub fn can_run(&self, cmd: &EditCommand, clipboard: &dyn crate::clipboard::Clipboard) -> bool {
        let editable = self.is_editable();
        let sel = self.selection.get().normalize();
        match cmd {
            EditCommand::Copy => self.may_copy(),
            EditCommand::Cut => editable && self.may_copy(),
            EditCommand::Paste => editable && clipboard.read_text().is_some_and(|t| !t.is_empty()),
            EditCommand::SelectAll => true,
            EditCommand::Undo => self.can_undo(),
            EditCommand::Redo => self.can_redo(),
            EditCommand::InsertText(_) => editable,
            EditCommand::DeleteWordBackward => editable && (!sel.is_collapsed() || sel.start > 0),
            EditCommand::DeleteWordForward => {
                editable && (!sel.is_collapsed() || sel.end < self.value.borrow().len())
            }
        }
    }

    /// Run `cmd` on this control. Copy and cut write to `clipboard`, paste
    /// reads it.
    pub fn run_command(
        &self,
        cmd: &EditCommand,
        clipboard: &dyn crate::clipboard::Clipboard,
    ) -> EditOutcome {
        let before_sel = (self.selection.get(), self.selection_direction.get());
        let mut data = None;
        let (handled, changed) = match cmd {
            // Enabled with an empty selection too (it copies nothing), but
            // never in a password field.
            EditCommand::Copy => {
                self.copy(clipboard);
                (self.kind.get() != TextControlKind::Password, false)
            }
            EditCommand::SelectAll => {
                self.select_all();
                (true, false)
            }
            EditCommand::Cut => {
                let done = self.cut(clipboard);
                (
                    self.is_editable() && self.kind.get() != TextControlKind::Password,
                    done,
                )
            }
            EditCommand::Paste => match clipboard.read_text() {
                Some(text) if self.is_editable() => {
                    data = self.insert_discrete(&text);
                    (true, data.is_some())
                }
                _ => (false, false),
            },
            EditCommand::Undo => {
                let done = self.undo();
                (done, done)
            }
            EditCommand::Redo => {
                let done = self.redo();
                (done, done)
            }
            EditCommand::InsertText(text) => {
                if !self.is_editable() {
                    (false, false)
                } else if text.is_empty() {
                    let sel = self.selection.get().normalize();
                    let done = self.delete_range(sel.start, sel.end);
                    (true, done)
                } else {
                    data = self.insert_discrete(text);
                    (true, data.is_some())
                }
            }
            EditCommand::DeleteWordBackward => {
                let done = self.delete_word_backward();
                (done, done)
            }
            EditCommand::DeleteWordForward => {
                let done = self.delete_word_forward();
                (done, done)
            }
        };
        let selection_changed =
            !changed && before_sel != (self.selection.get(), self.selection_direction.get());
        EditOutcome {
            handled,
            changed,
            selection_changed,
            input_type: if changed { cmd.input_type() } else { None },
            data: if changed { data } else { None },
        }
    }
}

/// The text control kind of `node`, `None` when it is not a text control
/// (an `<input>` whose type does not edit text, or another element).
pub fn text_control_kind(node: &crate::Node) -> Option<TextControlKind> {
    let crate::NodeType::Element { tag_name, .. } = &node.node_type else {
        return None;
    };
    if tag_name.eq_ignore_ascii_case("textarea") {
        return Some(TextControlKind::MultiLine);
    }
    if !tag_name.eq_ignore_ascii_case("input") {
        return None;
    }
    let ty = node
        .get_attribute("type")
        .unwrap_or("")
        .to_ascii_lowercase();
    match InputType::from_str(&ty) {
        InputType::Password => Some(TextControlKind::Password),
        t if t.is_text_input() && t != InputType::Number => Some(TextControlKind::SingleLine),
        _ => None,
    }
}

impl TextEditState {
    /// Take the control's editing attributes from its element: the kind
    /// (input type / textarea), `maxlength`, `readonly` and `disabled`.
    pub fn sync_attributes(&self, node: &crate::Node) {
        if let Some(kind) = text_control_kind(node) {
            self.set_kind(kind);
        }
        self.set_max_length(
            node.get_attribute("maxlength")
                .and_then(|v| v.trim().parse::<usize>().ok()),
        );
        self.set_read_only(node.get_attribute("readonly").is_some());
        self.set_disabled(node.get_attribute("disabled").is_some());
    }
}

/// State for checkbox/radio inputs.
#[derive(Debug, Default)]
pub struct CheckableState {
    /// Whether the input is checked.
    checked: Cell<bool>,
    /// Default checked state (for reset).
    default_checked: Cell<bool>,
    /// Whether indeterminate (checkboxes only).
    indeterminate: Cell<bool>,
}

impl CheckableState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_checked(&self) -> bool {
        self.checked.get()
    }

    pub fn set_checked(&self, checked: bool) {
        self.checked.set(checked);
        // Clear indeterminate when explicitly set
        self.indeterminate.set(false);
    }

    pub fn toggle(&self) {
        self.checked.set(!self.checked.get());
        self.indeterminate.set(false);
    }

    pub fn is_indeterminate(&self) -> bool {
        self.indeterminate.get()
    }

    pub fn set_indeterminate(&self, indeterminate: bool) {
        self.indeterminate.set(indeterminate);
    }

    pub fn reset(&self) {
        self.checked.set(self.default_checked.get());
        self.indeterminate.set(false);
    }
}

/// Form data entry for submission.
#[derive(Debug, Clone)]
pub struct FormDataEntry {
    pub name: String,
    pub value: FormDataValue,
}

/// Value types for form data.
#[derive(Debug, Clone)]
pub enum FormDataValue {
    String(String),
    File { name: String, content: Vec<u8> },
}

/// Encoding types for form submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormEnctype {
    #[default]
    UrlEncoded,
    MultipartFormData,
    TextPlain,
}

impl FormEnctype {
    /// Parse form enctype from string.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "multipart/form-data" => FormEnctype::MultipartFormData,
            "text/plain" => FormEnctype::TextPlain,
            _ => FormEnctype::UrlEncoded,
        }
    }

    pub fn content_type(&self) -> &'static str {
        match self {
            FormEnctype::UrlEncoded => "application/x-www-form-urlencoded",
            FormEnctype::MultipartFormData => "multipart/form-data",
            FormEnctype::TextPlain => "text/plain",
        }
    }
}

/// Form submission method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormMethod {
    #[default]
    Get,
    Post,
    Dialog,
}

impl FormMethod {
    /// Parse form method from string.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "post" => FormMethod::Post,
            "dialog" => FormMethod::Dialog,
            _ => FormMethod::Get,
        }
    }
}

/// Wrap mode for textarea elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// Lines wrap visually but are not wrapped in submitted data.
    #[default]
    Soft,
    /// Lines wrap visually and hard line breaks are inserted at wrap points on submit.
    Hard,
    /// No wrapping - horizontal scrolling for long lines.
    Off,
}

impl WrapMode {
    /// Parse wrap mode from string.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "hard" => WrapMode::Hard,
            "off" => WrapMode::Off,
            _ => WrapMode::Soft,
        }
    }
}

/// Line position in a textarea.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LinePosition {
    /// Zero-indexed line number.
    pub line: usize,
    /// Column (character offset within line).
    pub column: usize,
}

/// Text editing state for textarea elements with multi-line support.
#[derive(Debug)]
pub struct TextAreaState {
    /// Underlying text edit state.
    pub edit: TextEditState,
    /// Number of visible rows.
    rows: Cell<usize>,
    /// Number of visible columns (character width).
    cols: Cell<usize>,
    /// Wrap mode.
    wrap: Cell<WrapMode>,
}

impl Default for TextAreaState {
    fn default() -> Self {
        Self::new()
    }
}

impl TextAreaState {
    /// Create a new empty textarea state with default 2 rows, 20 cols.
    pub fn new() -> Self {
        let edit = TextEditState::new();
        edit.set_kind(TextControlKind::MultiLine);
        Self {
            edit,
            rows: Cell::new(2),
            cols: Cell::new(20),
            wrap: Cell::new(WrapMode::Soft),
        }
    }

    /// Create with initial value.
    pub fn with_value(value: impl Into<String>) -> Self {
        let edit = TextEditState::with_value(value);
        edit.set_kind(TextControlKind::MultiLine);
        Self {
            edit,
            rows: Cell::new(2),
            cols: Cell::new(20),
            wrap: Cell::new(WrapMode::Soft),
        }
    }

    /// Get the text value.
    pub fn value(&self) -> String {
        self.edit.value()
    }

    /// Set the text value.
    pub fn set_value(&self, value: impl Into<String>) {
        self.edit.set_value(value);
    }

    /// Get visible rows.
    pub fn rows(&self) -> usize {
        self.rows.get()
    }

    /// Set visible rows.
    pub fn set_rows(&self, rows: usize) {
        self.rows.set(rows.max(1));
    }

    /// Get visible columns.
    pub fn cols(&self) -> usize {
        self.cols.get()
    }

    /// Set visible columns.
    pub fn set_cols(&self, cols: usize) {
        self.cols.set(cols.max(1));
    }

    /// Get wrap mode.
    pub fn wrap(&self) -> WrapMode {
        self.wrap.get()
    }

    /// Set wrap mode.
    pub fn set_wrap(&self, wrap: WrapMode) {
        self.wrap.set(wrap);
    }

    /// Get lines as a vector of strings.
    pub fn lines(&self) -> Vec<String> {
        self.value().lines().map(|s| s.to_string()).collect()
    }

    /// Get the number of lines.
    pub fn line_count(&self) -> usize {
        self.lines().len().max(1)
    }

    /// Convert a character offset to line/column position.
    pub fn offset_to_position(&self, offset: usize) -> LinePosition {
        let value = self.edit.value();
        let offset = offset.min(value.len());

        let mut line = 0;
        let mut col = 0;
        let mut current_offset = 0;

        for ch in value.chars() {
            if current_offset >= offset {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
            current_offset += ch.len_utf8();
        }

        LinePosition { line, column: col }
    }

    /// Convert line/column position to character offset.
    pub fn position_to_offset(&self, pos: LinePosition) -> usize {
        let value = self.edit.value();
        let mut offset = 0;
        let mut current_line = 0;
        let mut current_col = 0;

        for ch in value.chars() {
            if current_line == pos.line && current_col >= pos.column {
                break;
            }
            if current_line > pos.line {
                break;
            }
            if ch == '\n' {
                if current_line == pos.line {
                    // Target position is at end of line
                    break;
                }
                current_line += 1;
                current_col = 0;
            } else {
                current_col += 1;
            }
            offset += ch.len_utf8();
        }

        offset
    }

    /// Get the current line position.
    pub fn caret_line_position(&self) -> LinePosition {
        self.offset_to_position(self.edit.focus_position())
    }

    /// Get the length of a specific line.
    pub fn line_length(&self, line: usize) -> usize {
        self.lines().get(line).map_or(0, |l| l.len())
    }

    /// Move caret up one line.
    pub fn move_up(&self, extend_selection: bool) {
        let pos = self.caret_line_position();
        if pos.line == 0 {
            // Already at first line
            self.edit.move_to_start(extend_selection);
            return;
        }

        let new_line = pos.line - 1;
        let new_col = pos.column.min(self.line_length(new_line));
        let new_offset = self.position_to_offset(LinePosition {
            line: new_line,
            column: new_col,
        });

        self.edit.move_caret_to(new_offset, extend_selection);
    }

    /// Move caret down one line.
    pub fn move_down(&self, extend_selection: bool) {
        let pos = self.caret_line_position();
        let line_count = self.line_count();

        if pos.line >= line_count.saturating_sub(1) {
            // Already at last line
            self.edit.move_to_end(extend_selection);
            return;
        }

        let new_line = pos.line + 1;
        let new_col = pos.column.min(self.line_length(new_line));
        let new_offset = self.position_to_offset(LinePosition {
            line: new_line,
            column: new_col,
        });

        self.edit.move_caret_to(new_offset, extend_selection);
    }

    /// Move caret to start of current line.
    pub fn move_to_line_start(&self, extend_selection: bool) {
        let pos = self.caret_line_position();
        let new_offset = self.position_to_offset(LinePosition {
            line: pos.line,
            column: 0,
        });

        self.edit.move_caret_to(new_offset, extend_selection);
    }

    /// Move caret to end of current line.
    pub fn move_to_line_end(&self, extend_selection: bool) {
        let pos = self.caret_line_position();
        let line_len = self.line_length(pos.line);
        let new_offset = self.position_to_offset(LinePosition {
            line: pos.line,
            column: line_len,
        });

        self.edit.move_caret_to(new_offset, extend_selection);
    }

    /// Insert a newline at the caret.
    pub fn insert_newline(&self) -> bool {
        self.edit.insert_text("\n")
    }

    /// Prepare value for form submission (apply hard wrapping if needed).
    pub fn submission_value(&self) -> String {
        match self.wrap.get() {
            WrapMode::Hard => {
                // In a real implementation, we'd wrap at visual wrap points
                // For now, just return the value as-is
                self.value()
            }
            _ => self.value(),
        }
    }
}

/// Form element state.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct FormState {
    /// Form action URL.
    action: RefCell<String>,
    /// Submission method.
    method: Cell<FormMethod>,
    /// Encoding type.
    enctype: Cell<FormEnctype>,
    /// Target frame/window.
    target: RefCell<String>,
    /// Whether to skip validation.
    novalidate: Cell<bool>,
    /// Form name.
    name: RefCell<String>,
}

impl FormState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn action(&self) -> String {
        self.action.borrow().clone()
    }

    pub fn set_action(&self, action: impl Into<String>) {
        *self.action.borrow_mut() = action.into();
    }

    pub fn method(&self) -> FormMethod {
        self.method.get()
    }

    pub fn set_method(&self, method: FormMethod) {
        self.method.set(method);
    }

    pub fn enctype(&self) -> FormEnctype {
        self.enctype.get()
    }

    pub fn set_enctype(&self, enctype: FormEnctype) {
        self.enctype.set(enctype);
    }

    pub fn target(&self) -> String {
        self.target.borrow().clone()
    }

    pub fn set_target(&self, target: impl Into<String>) {
        *self.target.borrow_mut() = target.into();
    }

    pub fn novalidate(&self) -> bool {
        self.novalidate.get()
    }

    pub fn set_novalidate(&self, novalidate: bool) {
        self.novalidate.set(novalidate);
    }

    /// Encode form data for submission.
    pub fn encode_form_data(entries: &[FormDataEntry], enctype: FormEnctype) -> Vec<u8> {
        match enctype {
            FormEnctype::UrlEncoded => Self::encode_url(entries),
            FormEnctype::TextPlain => Self::encode_text_plain(entries),
            FormEnctype::MultipartFormData => Self::encode_multipart(entries),
        }
    }

    fn encode_url(entries: &[FormDataEntry]) -> Vec<u8> {
        let parts: Vec<String> = entries
            .iter()
            .filter_map(|e| {
                if let FormDataValue::String(v) = &e.value {
                    Some(format!(
                        "{}={}",
                        urlencoding::encode(&e.name),
                        urlencoding::encode(v)
                    ))
                } else {
                    None
                }
            })
            .collect();
        parts.join("&").into_bytes()
    }

    fn encode_text_plain(entries: &[FormDataEntry]) -> Vec<u8> {
        let parts: Vec<String> = entries
            .iter()
            .filter_map(|e| {
                if let FormDataValue::String(v) = &e.value {
                    Some(format!("{}={}", e.name, v))
                } else {
                    None
                }
            })
            .collect();
        parts.join("\r\n").into_bytes()
    }

    fn encode_multipart(entries: &[FormDataEntry]) -> Vec<u8> {
        // Simplified multipart - real implementation would use proper boundary
        let boundary = "----RustKitFormBoundary";
        let mut result = Vec::new();

        for entry in entries {
            result.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());

            match &entry.value {
                FormDataValue::String(v) => {
                    result.extend_from_slice(
                        format!(
                            "Content-Disposition: form-data; name=\"{}\"\r\n\r\n{}\r\n",
                            entry.name, v
                        )
                        .as_bytes(),
                    );
                }
                FormDataValue::File { name, content } => {
                    result.extend_from_slice(
                        format!(
                            "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n\
                             Content-Type: application/octet-stream\r\n\r\n",
                            entry.name, name
                        )
                        .as_bytes(),
                    );
                    result.extend_from_slice(content);
                    result.extend_from_slice(b"\r\n");
                }
            }
        }

        result.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());
        result
    }

    /// Create a submission request from form data.
    pub fn create_submission(&self, base_url: &str, entries: &[FormDataEntry]) -> FormSubmission {
        let action = self.action();
        let method = self.method();
        let enctype = self.enctype();
        let target = self.target();

        // Resolve action URL relative to base
        let url = if action.is_empty() {
            base_url.to_string()
        } else {
            resolve_url(base_url, &action).unwrap_or(action)
        };

        match method {
            FormMethod::Get => {
                // Append data to URL as query string
                let query = String::from_utf8_lossy(&FormState::encode_url(entries)).to_string();
                let final_url = match url::Url::parse(&url) {
                    // Into the query, ahead of any fragment.
                    Ok(mut parsed) => {
                        let joined = match parsed.query() {
                            Some(existing) => format!("{}&{}", existing, query),
                            None => query,
                        };
                        parsed.set_query(Some(&joined));
                        parsed.to_string()
                    }
                    Err(_) if url.contains('?') => format!("{}&{}", url, query),
                    Err(_) => format!("{}?{}", url, query),
                };

                FormSubmission {
                    url: final_url,
                    method: FormMethod::Get,
                    content_type: None,
                    body: None,
                    target,
                }
            }
            FormMethod::Post => {
                let body = FormState::encode_form_data(entries, enctype);
                let content_type = match enctype {
                    FormEnctype::UrlEncoded => "application/x-www-form-urlencoded".to_string(),
                    FormEnctype::MultipartFormData => {
                        "multipart/form-data; boundary=----RustKitFormBoundary".to_string()
                    }
                    FormEnctype::TextPlain => "text/plain".to_string(),
                };

                FormSubmission {
                    url,
                    method: FormMethod::Post,
                    content_type: Some(content_type),
                    body: Some(body),
                    target,
                }
            }
            FormMethod::Dialog => {
                // Dialog method closes the dialog with the return value
                FormSubmission {
                    url,
                    method: FormMethod::Dialog,
                    content_type: None,
                    body: None,
                    target,
                }
            }
        }
    }
}

/// `input` resolved against `base` as the URL Standard says (`//host/path`
/// keeps only the base's scheme), or `None` when either does not parse.
pub fn resolve_url(base: &str, input: &str) -> Option<String> {
    url::Url::parse(base).ok()?.join(input).ok().map(String::from)
}

/// A prepared form submission ready to be sent.
#[derive(Debug, Clone)]
pub struct FormSubmission {
    /// The URL to submit to.
    pub url: String,
    /// The HTTP method.
    pub method: FormMethod,
    /// Content-Type header value (for POST).
    pub content_type: Option<String>,
    /// Request body (for POST).
    pub body: Option<Vec<u8>>,
    /// Target frame/window.
    pub target: String,
}

impl FormSubmission {
    /// Check if this submission should replace the current page.
    pub fn is_self_target(&self) -> bool {
        self.target.is_empty() || self.target == "_self" || self.target.to_lowercase() == "self"
    }

    /// Check if this submission should open in a new window/tab.
    pub fn is_blank_target(&self) -> bool {
        self.target == "_blank"
    }

    /// Check if this is a GET request.
    pub fn is_get(&self) -> bool {
        self.method == FormMethod::Get
    }

    /// Check if this is a POST request.
    pub fn is_post(&self) -> bool {
        self.method == FormMethod::Post
    }

    /// Check if this is a dialog close action.
    pub fn is_dialog(&self) -> bool {
        self.method == FormMethod::Dialog
    }

    /// Get the body as a string (for debugging/logging).
    pub fn body_as_string(&self) -> Option<String> {
        self.body
            .as_ref()
            .map(|b| String::from_utf8_lossy(b).to_string())
    }
}

/// Result of handling a keyboard event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyHandleResult {
    /// Event was handled, value changed.
    ValueChanged,
    /// Event was handled, selection changed.
    SelectionChanged,
    /// Event triggers form submission.
    Submit,
    /// Event not handled (propagate to parent).
    Unhandled,
    /// Event handled but no change.
    Handled,
}

/// Handle keyboard events for text input fields.
/// This module bridges key events from rustkit-core to TextEditState operations.
pub mod keyboard {
    use super::*;
    use crate::clipboard::Clipboard;

    const VK_BACK: u32 = 0x08;
    const VK_TAB: u32 = 0x09;
    const VK_RETURN: u32 = 0x0D;
    const VK_ESCAPE: u32 = 0x1B;
    const VK_END: u32 = 0x23;
    const VK_HOME: u32 = 0x24;
    const VK_LEFT: u32 = 0x25;
    const VK_UP: u32 = 0x26;
    const VK_RIGHT: u32 = 0x27;
    const VK_DOWN: u32 = 0x28;
    const VK_DELETE: u32 = 0x2E;

    /// The letter a chord names: the Win32 VK for A-Z, or the typed text
    /// (the window sends Cmd+C as vk 0 with text "c", or "C" with Shift).
    fn chord_letter(key_code: u32, key: &str) -> Option<char> {
        if (0x41..=0x5A).contains(&key_code) {
            return char::from_u32(key_code).map(|c| c.to_ascii_lowercase());
        }
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
            _ => None,
        }
    }

    /// The edit command a key chord means on `platform`, if it is one
    /// (clipboard, history, select-all, delete-word). `primary` is the
    /// command modifier: Cmd on macOS, Ctrl elsewhere.
    pub fn command_for_key(
        platform: KeyPlatform,
        key_code: u32,
        key: &str,
        primary: bool,
        shift: bool,
        alt: bool,
    ) -> Option<EditCommand> {
        if primary && !alt {
            if let Some(letter) = chord_letter(key_code, key) {
                return match (letter, shift) {
                    ('a', false) => Some(EditCommand::SelectAll),
                    ('c', _) => Some(EditCommand::Copy),
                    ('x', _) => Some(EditCommand::Cut),
                    ('v', _) => Some(EditCommand::Paste),
                    ('z', false) => Some(EditCommand::Undo),
                    ('z', true) => Some(EditCommand::Redo),
                    ('y', false) if platform == KeyPlatform::Other => Some(EditCommand::Redo),
                    _ => None,
                };
            }
        }
        // Delete by word: Option+Backspace/Delete on macOS, Ctrl elsewhere.
        let word_mod = match platform {
            KeyPlatform::Mac => alt && !primary,
            KeyPlatform::Other => primary && !alt,
        };
        match key_code {
            VK_BACK if word_mod => Some(EditCommand::DeleteWordBackward),
            VK_DELETE if word_mod => Some(EditCommand::DeleteWordForward),
            _ => None,
        }
    }

    /// An outcome as the key handler's result.
    fn result_of(out: &EditOutcome) -> KeyHandleResult {
        if out.changed {
            KeyHandleResult::ValueChanged
        } else if out.selection_changed {
            KeyHandleResult::SelectionChanged
        } else {
            KeyHandleResult::Handled
        }
    }

    /// Handle a key event for a single-line text input, on this platform,
    /// copying to and pasting from this thread's default in-memory
    /// clipboard. Returns what action was taken.
    pub fn handle_input_key(
        state: &TextEditState,
        key_code: u32,
        key: &str,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> KeyHandleResult {
        let clipboard = crate::clipboard::thread_default();
        handle_key(
            state,
            None,
            &*clipboard,
            KeyPlatform::current(),
            key_code,
            key,
            ctrl,
            shift,
            alt,
        )
    }

    /// Handle a key event for a multi-line textarea (see
    /// [`handle_input_key`]).
    pub fn handle_textarea_key(
        state: &TextAreaState,
        key_code: u32,
        key: &str,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> KeyHandleResult {
        let clipboard = crate::clipboard::thread_default();
        handle_key(
            &state.edit,
            Some(state),
            &*clipboard,
            KeyPlatform::current(),
            key_code,
            key,
            ctrl,
            shift,
            alt,
        )
    }

    /// A key for a text control, with the clipboard and platform explicit.
    /// `area` is the textarea wrapping `state`, for line movement and
    /// Enter. `primary` is the command modifier (Cmd on macOS, Ctrl
    /// elsewhere; the window passes Ctrl-or-Cmd).
    #[allow(clippy::too_many_arguments)]
    pub fn handle_key(
        state: &TextEditState,
        area: Option<&TextAreaState>,
        clipboard: &dyn Clipboard,
        platform: KeyPlatform,
        key_code: u32,
        key: &str,
        primary: bool,
        shift: bool,
        alt: bool,
    ) -> KeyHandleResult {
        if let Some(cmd) = command_for_key(platform, key_code, key, primary, shift, alt) {
            return result_of(&state.run_command(&cmd, clipboard));
        }
        let mac = platform == KeyPlatform::Mac;
        // Moving by word: Option+arrow on macOS, Ctrl+arrow elsewhere. Cmd
        // +arrow on macOS goes to the line's (or the text's) ends.
        let by_word = if mac {
            alt && !primary
        } else {
            primary && !alt
        };
        let to_edge = mac && primary && !alt;
        let moved = |f: &dyn Fn()| {
            f();
            KeyHandleResult::SelectionChanged
        };
        match key_code {
            VK_LEFT if by_word => return moved(&|| state.move_word_left(shift)),
            VK_RIGHT if by_word => return moved(&|| state.move_word_right(shift, platform)),
            VK_LEFT if to_edge => {
                return moved(&|| match area {
                    Some(a) => a.move_to_line_start(shift),
                    None => state.move_to_start(shift),
                })
            }
            VK_RIGHT if to_edge => {
                return moved(&|| match area {
                    Some(a) => a.move_to_line_end(shift),
                    None => state.move_to_end(shift),
                })
            }
            // Cmd+Up/Down (macOS) and Ctrl+Home/End: the text's ends.
            VK_UP if to_edge => return moved(&|| state.move_to_start(shift)),
            VK_DOWN if to_edge => return moved(&|| state.move_to_end(shift)),
            VK_HOME if primary && !alt => return moved(&|| state.move_to_start(shift)),
            VK_END if primary && !alt => return moved(&|| state.move_to_end(shift)),
            _ => {}
        }
        // Other chords are the browser's or the page's (and Option+letter
        // composition is not modelled).
        if primary || alt {
            return KeyHandleResult::Unhandled;
        }

        match key_code {
            VK_LEFT => moved(&|| state.move_left(shift)),
            VK_RIGHT => moved(&|| state.move_right(shift)),
            VK_UP if area.is_some() => moved(&|| area.unwrap().move_up(shift)),
            VK_DOWN if area.is_some() => moved(&|| area.unwrap().move_down(shift)),
            VK_HOME => moved(&|| match area {
                Some(a) => a.move_to_line_start(shift),
                None => state.move_to_start(shift),
            }),
            VK_END => moved(&|| match area {
                Some(a) => a.move_to_line_end(shift),
                None => state.move_to_end(shift),
            }),
            VK_BACK => {
                if state.delete_backward() {
                    KeyHandleResult::ValueChanged
                } else {
                    KeyHandleResult::Handled
                }
            }
            VK_DELETE => {
                if state.delete_forward() {
                    KeyHandleResult::ValueChanged
                } else {
                    KeyHandleResult::Handled
                }
            }
            VK_RETURN => match area {
                // Enter inserts a line break in a textarea...
                Some(a) => {
                    if a.insert_newline() {
                        KeyHandleResult::ValueChanged
                    } else {
                        KeyHandleResult::Handled
                    }
                }
                // ...and submits the form from a single-line input.
                None => KeyHandleResult::Submit,
            },
            // Tab moves focus; Escape typically blurs.
            VK_TAB | VK_ESCAPE => KeyHandleResult::Unhandled,
            // Character input
            _ => {
                let mut chars = key.chars();
                match (chars.next(), chars.next()) {
                    (Some(ch), None) if !ch.is_control() => {
                        if state.insert_text(key) {
                            KeyHandleResult::ValueChanged
                        } else {
                            KeyHandleResult::Handled
                        }
                    }
                    _ => KeyHandleResult::Unhandled,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selection_range() {
        let sel = SelectionRange::new(5, 10);
        assert_eq!(sel.len(), 5);
        assert!(!sel.is_collapsed());

        let caret = SelectionRange::caret(5);
        assert!(caret.is_collapsed());
        assert_eq!(caret.len(), 0);

        // Normalize reversed selection
        let reversed = SelectionRange::new(10, 5);
        let normalized = reversed.normalize();
        assert_eq!(normalized.start, 5);
        assert_eq!(normalized.end, 10);
    }

    #[test]
    fn test_text_edit_insert() {
        let state = TextEditState::new();
        state.insert_text("Hello");
        assert_eq!(state.value(), "Hello");
        assert_eq!(state.caret_position(), 5);

        state.insert_text(" World");
        assert_eq!(state.value(), "Hello World");
        assert_eq!(state.caret_position(), 11);
    }

    #[test]
    fn test_text_edit_delete_backward() {
        let state = TextEditState::with_value("Hello");
        state.set_caret(5);

        state.delete_backward();
        assert_eq!(state.value(), "Hell");
        assert_eq!(state.caret_position(), 4);

        state.delete_backward();
        assert_eq!(state.value(), "Hel");
    }

    #[test]
    fn test_text_edit_delete_forward() {
        let state = TextEditState::with_value("Hello");
        state.set_caret(0);

        state.delete_forward();
        assert_eq!(state.value(), "ello");
        assert_eq!(state.caret_position(), 0);
    }

    #[test]
    fn test_text_edit_selection_replace() {
        let state = TextEditState::with_value("Hello World");
        state.set_selection(6, 11);

        state.insert_text("Rust");
        assert_eq!(state.value(), "Hello Rust");
        assert_eq!(state.caret_position(), 10);
    }

    #[test]
    fn test_text_edit_move_caret() {
        let state = TextEditState::with_value("Hello");
        state.set_caret(2);

        state.move_left(false);
        assert_eq!(state.caret_position(), 1);

        state.move_right(false);
        assert_eq!(state.caret_position(), 2);

        state.move_to_start(false);
        assert_eq!(state.caret_position(), 0);

        state.move_to_end(false);
        assert_eq!(state.caret_position(), 5);
    }

    #[test]
    fn test_text_edit_extend_selection() {
        let state = TextEditState::with_value("Hello");
        state.set_caret(2);

        state.move_right(true);
        let sel = state.selection();
        assert_eq!(sel.start, 2);
        assert_eq!(sel.end, 3);

        state.move_right(true);
        let sel = state.selection();
        assert_eq!(sel.start, 2);
        assert_eq!(sel.end, 4);
    }

    #[test]
    fn test_text_edit_max_length() {
        let state = TextEditState::new();
        state.set_max_length(Some(5));

        state.insert_text("Hello");
        assert!(!state.insert_text("!"));
        assert_eq!(state.value(), "Hello");
    }

    #[test]
    fn test_text_edit_readonly() {
        let state = TextEditState::with_value("Hello");
        state.set_read_only(true);

        assert!(!state.insert_text(" World"));
        assert_eq!(state.value(), "Hello");
    }

    #[test]
    fn test_input_type_parsing() {
        assert_eq!(InputType::from_str("text"), InputType::Text);
        assert_eq!(InputType::from_str("PASSWORD"), InputType::Password);
        assert_eq!(InputType::from_str("checkbox"), InputType::Checkbox);
        assert_eq!(InputType::from_str("unknown"), InputType::Text);

        assert!(InputType::Text.is_text_input());
        assert!(!InputType::Checkbox.is_text_input());
        assert!(InputType::Submit.is_button());
        assert!(InputType::Checkbox.is_checkable());
    }

    #[test]
    fn test_checkable_state() {
        let state = CheckableState::new();
        assert!(!state.is_checked());

        state.set_checked(true);
        assert!(state.is_checked());

        state.toggle();
        assert!(!state.is_checked());
    }

    #[test]
    fn test_form_data_encoding() {
        let entries = vec![
            FormDataEntry {
                name: "name".to_string(),
                value: FormDataValue::String("John Doe".to_string()),
            },
            FormDataEntry {
                name: "email".to_string(),
                value: FormDataValue::String("john@example.com".to_string()),
            },
        ];

        let encoded = FormState::encode_form_data(&entries, FormEnctype::UrlEncoded);
        let encoded_str = String::from_utf8(encoded).unwrap();
        assert!(encoded_str.contains("name=John%20Doe"));
        assert!(encoded_str.contains("email=john%40example.com"));
    }

    #[test]
    fn test_validation() {
        let state = TextEditState::new();
        state.set_required(true);
        assert!(!state.is_valid());

        state.insert_text("hello");
        assert!(state.is_valid());

        state.set_min_length(10);
        assert!(!state.is_valid());

        state.insert_text(" world!");
        assert!(state.is_valid());
    }

    #[test]
    fn test_textarea_basic() {
        let state = TextAreaState::with_value("Hello\nWorld");
        assert_eq!(state.line_count(), 2);
        assert_eq!(state.lines(), vec!["Hello", "World"]);
        assert_eq!(state.rows(), 2);
        assert_eq!(state.cols(), 20);
    }

    #[test]
    fn test_textarea_position_conversion() {
        let state = TextAreaState::with_value("Hello\nWorld\nTest");

        // "Hello\n" = 6 chars, "World\n" = 6 chars
        let pos = state.offset_to_position(0);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.column, 0);

        let pos = state.offset_to_position(3);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.column, 3);

        let pos = state.offset_to_position(6); // Start of "World"
        assert_eq!(pos.line, 1);
        assert_eq!(pos.column, 0);

        let pos = state.offset_to_position(12); // Start of "Test"
        assert_eq!(pos.line, 2);
        assert_eq!(pos.column, 0);

        // Roundtrip
        assert_eq!(
            state.position_to_offset(LinePosition { line: 0, column: 3 }),
            3
        );
        assert_eq!(
            state.position_to_offset(LinePosition { line: 1, column: 0 }),
            6
        );
        assert_eq!(
            state.position_to_offset(LinePosition { line: 2, column: 2 }),
            14
        );
    }

    #[test]
    fn test_textarea_line_navigation() {
        let state = TextAreaState::with_value("Hello\nWorld\nTest");
        state.edit.set_caret(3); // In "Hello" at position 3

        state.move_down(false);
        let pos = state.caret_line_position();
        assert_eq!(pos.line, 1);
        assert_eq!(pos.column, 3);

        state.move_down(false);
        let pos = state.caret_line_position();
        assert_eq!(pos.line, 2);
        // "Test" is only 4 chars, column clamped to 3
        assert!(pos.column <= 3);

        state.move_up(false);
        let pos = state.caret_line_position();
        assert_eq!(pos.line, 1);
    }

    #[test]
    fn test_textarea_line_start_end() {
        let state = TextAreaState::with_value("Hello\nWorld\nTest");
        state.edit.set_caret(8); // In "World" somewhere

        state.move_to_line_start(false);
        let pos = state.caret_line_position();
        assert_eq!(pos.column, 0);
        assert_eq!(pos.line, 1);

        state.move_to_line_end(false);
        let pos = state.caret_line_position();
        assert_eq!(pos.column, 5); // "World" has 5 chars
        assert_eq!(pos.line, 1);
    }

    #[test]
    fn test_textarea_insert_newline() {
        let state = TextAreaState::with_value("HelloWorld");
        state.edit.set_caret(5);

        state.insert_newline();
        assert_eq!(state.value(), "Hello\nWorld");
        assert_eq!(state.line_count(), 2);
    }

    #[test]
    fn test_wrap_mode() {
        assert_eq!(WrapMode::from_str("soft"), WrapMode::Soft);
        assert_eq!(WrapMode::from_str("hard"), WrapMode::Hard);
        assert_eq!(WrapMode::from_str("off"), WrapMode::Off);
        assert_eq!(WrapMode::from_str("unknown"), WrapMode::Soft);
    }

    #[test]
    fn test_keyboard_input_character() {
        let state = TextEditState::new();

        // Type "Hello"
        let result = keyboard::handle_input_key(&state, 0x48, "H", false, true, false);
        assert_eq!(result, KeyHandleResult::ValueChanged);
        let result = keyboard::handle_input_key(&state, 0x45, "e", false, false, false);
        assert_eq!(result, KeyHandleResult::ValueChanged);
        let result = keyboard::handle_input_key(&state, 0x4C, "l", false, false, false);
        assert_eq!(result, KeyHandleResult::ValueChanged);
        let result = keyboard::handle_input_key(&state, 0x4C, "l", false, false, false);
        assert_eq!(result, KeyHandleResult::ValueChanged);
        let result = keyboard::handle_input_key(&state, 0x4F, "o", false, false, false);
        assert_eq!(result, KeyHandleResult::ValueChanged);

        assert_eq!(state.value(), "Hello");
    }

    #[test]
    fn test_keyboard_input_navigation() {
        let state = TextEditState::with_value("Hello");
        state.set_caret(2);

        // Arrow left
        let result = keyboard::handle_input_key(&state, 0x25, "", false, false, false);
        assert_eq!(result, KeyHandleResult::SelectionChanged);
        assert_eq!(state.caret_position(), 1);

        // Arrow right
        let result = keyboard::handle_input_key(&state, 0x27, "", false, false, false);
        assert_eq!(result, KeyHandleResult::SelectionChanged);
        assert_eq!(state.caret_position(), 2);

        // Home
        let result = keyboard::handle_input_key(&state, 0x24, "", false, false, false);
        assert_eq!(result, KeyHandleResult::SelectionChanged);
        assert_eq!(state.caret_position(), 0);

        // End
        let result = keyboard::handle_input_key(&state, 0x23, "", false, false, false);
        assert_eq!(result, KeyHandleResult::SelectionChanged);
        assert_eq!(state.caret_position(), 5);
    }

    #[test]
    fn test_keyboard_input_deletion() {
        let state = TextEditState::with_value("Hello");
        state.set_caret(5);

        // Backspace
        let result = keyboard::handle_input_key(&state, 0x08, "", false, false, false);
        assert_eq!(result, KeyHandleResult::ValueChanged);
        assert_eq!(state.value(), "Hell");

        state.set_caret(0);
        // Delete
        let result = keyboard::handle_input_key(&state, 0x2E, "", false, false, false);
        assert_eq!(result, KeyHandleResult::ValueChanged);
        assert_eq!(state.value(), "ell");
    }

    #[test]
    fn test_keyboard_input_enter_submit() {
        let state = TextEditState::new();

        let result = keyboard::handle_input_key(&state, 0x0D, "", false, false, false);
        assert_eq!(result, KeyHandleResult::Submit);
    }

    #[test]
    fn test_keyboard_ctrl_a_select_all() {
        let state = TextEditState::with_value("Hello World");
        state.set_caret(0);

        let result = keyboard::handle_input_key(&state, 0x41, "a", true, false, false);
        assert_eq!(result, KeyHandleResult::SelectionChanged);

        let sel = state.selection();
        assert_eq!(sel.start, 0);
        assert_eq!(sel.end, 11);
    }

    #[test]
    fn test_keyboard_textarea_enter() {
        let state = TextAreaState::with_value("Hello");
        state.edit.set_caret(5);

        let result = keyboard::handle_textarea_key(&state, 0x0D, "", false, false, false);
        assert_eq!(result, KeyHandleResult::ValueChanged);
        assert_eq!(state.value(), "Hello\n");
    }

    #[test]
    fn test_keyboard_textarea_arrows() {
        let state = TextAreaState::with_value("Line1\nLine2\nLine3");
        state.edit.set_caret(8); // In "Line2"

        // Arrow up
        let result = keyboard::handle_textarea_key(&state, 0x26, "", false, false, false);
        assert_eq!(result, KeyHandleResult::SelectionChanged);
        let pos = state.caret_line_position();
        assert_eq!(pos.line, 0);

        // Arrow down
        let result = keyboard::handle_textarea_key(&state, 0x28, "", false, false, false);
        assert_eq!(result, KeyHandleResult::SelectionChanged);
        let pos = state.caret_line_position();
        assert_eq!(pos.line, 1);
    }

    #[test]
    fn test_keyboard_shift_selection() {
        let state = TextEditState::with_value("Hello");
        state.set_caret(2);

        // Shift+Right should extend selection
        let result = keyboard::handle_input_key(&state, 0x27, "", false, true, false);
        assert_eq!(result, KeyHandleResult::SelectionChanged);

        let sel = state.selection();
        assert_eq!(sel.start, 2);
        assert_eq!(sel.end, 3);
    }

    #[test]
    fn test_keyboard_alt_unhandled() {
        let state = TextEditState::new();

        // Alt+key should not be handled (browser shortcut)
        let result = keyboard::handle_input_key(&state, 0x41, "a", false, false, true);
        assert_eq!(result, KeyHandleResult::Unhandled);
    }

    #[test]
    fn test_form_submission_get() {
        let form = FormState::new();
        form.set_action("/search");
        form.set_method(FormMethod::Get);

        let entries = vec![FormDataEntry {
            name: "q".to_string(),
            value: FormDataValue::String("hello world".to_string()),
        }];

        let submission = form.create_submission("https://example.com/page", &entries);
        assert!(submission.is_get());
        assert!(submission.url.contains("/search?q=hello%20world"));
        assert!(submission.body.is_none());
    }

    /// A GET with no successful controls still builds `action?` (HTML form
    /// submission). Engine #771 depended on this shape after the empty-list
    /// early return was removed.
    #[test]
    fn a_get_with_no_entries_still_appends_an_empty_query() {
        let form = FormState::new();
        form.set_action("/search");
        form.set_method(FormMethod::Get);
        let submission = form.create_submission("https://example.com/page", &[]);
        assert_eq!(submission.url, "https://example.com/search?");
        assert!(submission.body.is_none());
    }

    #[test]
    fn test_form_submission_post() {
        let form = FormState::new();
        form.set_action("/login");
        form.set_method(FormMethod::Post);
        form.set_enctype(FormEnctype::UrlEncoded);

        let entries = vec![
            FormDataEntry {
                name: "username".to_string(),
                value: FormDataValue::String("user".to_string()),
            },
            FormDataEntry {
                name: "password".to_string(),
                value: FormDataValue::String("pass123".to_string()),
            },
        ];

        let submission = form.create_submission("https://example.com/", &entries);
        assert!(submission.is_post());
        assert_eq!(submission.url, "https://example.com/login");
        assert!(submission.content_type.is_some());
        assert!(submission.body.is_some());

        let body = submission.body_as_string().unwrap();
        assert!(body.contains("username=user"));
        assert!(body.contains("password=pass123"));
    }

    #[test]
    fn test_form_submission_target() {
        let form = FormState::new();
        form.set_target("_blank");

        let submission = form.create_submission("https://example.com/", &[]);
        assert!(submission.is_blank_target());
        assert!(!submission.is_self_target());

        form.set_target("");
        let submission = form.create_submission("https://example.com/", &[]);
        assert!(submission.is_self_target());
    }

    /// A protocol-relative action (`//host/path`) takes only its scheme
    /// from the document (WHATWG URL): the Wikipedia portal's search form.
    #[test]
    fn test_form_submission_protocol_relative_action() {
        let base = "https://www.wikipedia.org/";
        let submit = |action: &str, method: FormMethod| {
            let form = FormState::new();
            form.set_action(action);
            form.set_method(method);
            form.create_submission(base, &[]).url
        };
        assert_eq!(
            submit("//www.wikipedia.org/search-redirect.php", FormMethod::Post),
            "https://www.wikipedia.org/search-redirect.php"
        );
        assert_eq!(submit("//example.test", FormMethod::Post), "https://example.test/");
        assert_eq!(
            submit("//example.test/path?q#f", FormMethod::Post),
            "https://example.test/path?q#f"
        );
        assert_eq!(
            form_with_entry("//www.wikipedia.org/search-redirect.php", base),
            "https://www.wikipedia.org/search-redirect.php?search=rust"
        );
    }

    /// The other relative forms resolve as the URL Standard says too.
    #[test]
    fn test_form_submission_relative_actions() {
        let base = "https://example.com/dir/page?old#frag";
        let submit = |action: &str| {
            let form = FormState::new();
            form.set_action(action);
            form.set_method(FormMethod::Post);
            form.create_submission(base, &[]).url
        };
        assert_eq!(submit("next"), "https://example.com/dir/next");
        assert_eq!(submit("../up"), "https://example.com/up");
        assert_eq!(submit("?q=1"), "https://example.com/dir/page?q=1");
        assert_eq!(submit("http://other.test/x"), "http://other.test/x");
        assert_eq!(submit(""), base);
    }

    /// A GET submission of one `search=rust` pair to `action`.
    fn form_with_entry(action: &str, base: &str) -> String {
        let form = FormState::new();
        form.set_action(action);
        form.set_method(FormMethod::Get);
        let entries = vec![FormDataEntry {
            name: "search".to_string(),
            value: FormDataValue::String("rust".to_string()),
        }];
        form.create_submission(base, &entries).url
    }

    #[test]
    fn test_resolve_url() {
        let base = "https://www.wikipedia.org/";
        assert_eq!(resolve_url(base, "//example.test/path").as_deref(), Some("https://example.test/path"));
        assert_eq!(resolve_url(base, "//example.test").as_deref(), Some("https://example.test/"));
        assert_eq!(resolve_url(base, "//example.test/p?q#f").as_deref(), Some("https://example.test/p?q#f"));
        assert_eq!(resolve_url("http://localhost:8080/a/b", "c").as_deref(), Some("http://localhost:8080/a/c"));
        assert_eq!(resolve_url("invalid", "/x"), None);
    }
}

#[cfg(test)]
mod edit_command_tests {
    //! CH2.E1: clipboard, cut/copy/paste, undo/redo and word movement in
    //! text controls, through the same key entry point the engine uses.
    use super::keyboard::{command_for_key, handle_key};
    use super::*;
    use crate::clipboard::{Clipboard, MemoryClipboard};

    const VK_BACK: u32 = 0x08;
    const VK_DELETE: u32 = 0x2E;
    const VK_LEFT: u32 = 0x25;
    const VK_RIGHT: u32 = 0x27;
    const VK_HOME: u32 = 0x24;
    const VK_END: u32 = 0x23;

    /// A key on `platform` with the command modifier held (Cmd/Ctrl).
    fn cmd(
        s: &TextEditState,
        cb: &dyn Clipboard,
        p: KeyPlatform,
        letter: &str,
        shift: bool,
    ) -> KeyHandleResult {
        // The window delivers Cmd+C as vk 0 + text "c" (hiwave-app).
        handle_key(s, None, cb, p, 0, letter, true, shift, false)
    }

    fn key(
        s: &TextEditState,
        cb: &dyn Clipboard,
        p: KeyPlatform,
        vk: u32,
        shift: bool,
        primary: bool,
        alt: bool,
    ) -> KeyHandleResult {
        handle_key(s, None, cb, p, vk, "", primary, shift, alt)
    }

    fn typed(s: &TextEditState, cb: &dyn Clipboard, text: &str) {
        for ch in text.chars() {
            let k = ch.to_string();
            assert_eq!(
                handle_key(s, None, cb, KeyPlatform::Mac, 0, &k, false, false, false),
                KeyHandleResult::ValueChanged
            );
        }
    }

    #[test]
    fn select_all_copy_end_paste_doubles_the_text() {
        for p in [KeyPlatform::Mac, KeyPlatform::Other] {
            let cb = MemoryClipboard::new();
            let s = TextEditState::with_value("hello");
            assert_eq!(
                cmd(&s, &cb, p, "a", false),
                KeyHandleResult::SelectionChanged
            );
            assert_eq!(s.selected_text(), "hello");
            assert_eq!(cmd(&s, &cb, p, "c", false), KeyHandleResult::Handled);
            assert_eq!(cb.read_text().as_deref(), Some("hello"));
            assert_eq!(
                key(&s, &cb, p, VK_END, false, false, false),
                KeyHandleResult::SelectionChanged
            );
            assert_eq!(cmd(&s, &cb, p, "v", false), KeyHandleResult::ValueChanged);
            assert_eq!(s.value(), "hellohello", "{p:?}");
            assert_eq!(s.selection(), SelectionRange::caret(10));
        }
    }

    #[test]
    fn cut_empties_the_field_and_fills_the_clipboard() {
        let cb = MemoryClipboard::new();
        let s = TextEditState::with_value("hello");
        cmd(&s, &cb, KeyPlatform::Other, "a", false);
        assert_eq!(
            cmd(&s, &cb, KeyPlatform::Other, "x", false),
            KeyHandleResult::ValueChanged
        );
        assert_eq!(s.value(), "");
        assert_eq!(cb.read_text().as_deref(), Some("hello"));
        // Undo puts the cut text back, selected as it was.
        assert_eq!(
            cmd(&s, &cb, KeyPlatform::Other, "z", false),
            KeyHandleResult::ValueChanged
        );
        assert_eq!(s.value(), "hello");
        assert_eq!(s.selected_text(), "hello");
    }

    #[test]
    fn undo_after_typing_restores_empty_and_redo_restores_the_typing() {
        let cb = MemoryClipboard::new();
        let s = TextEditState::new();
        typed(&s, &cb, "abc");
        // Consecutive typed characters are one history entry.
        assert_eq!(
            cmd(&s, &cb, KeyPlatform::Mac, "z", false),
            KeyHandleResult::ValueChanged
        );
        assert_eq!(s.value(), "");
        assert!(!s.can_undo());
        // Cmd+Shift+Z (mac) and Ctrl+Y (elsewhere) redo.
        assert_eq!(
            cmd(&s, &cb, KeyPlatform::Mac, "Z", true),
            KeyHandleResult::ValueChanged
        );
        assert_eq!(s.value(), "abc");
        assert_eq!(s.caret_position(), 3);
        cmd(&s, &cb, KeyPlatform::Other, "z", false);
        assert_eq!(s.value(), "");
        assert_eq!(
            cmd(&s, &cb, KeyPlatform::Other, "y", false),
            KeyHandleResult::ValueChanged
        );
        assert_eq!(s.value(), "abc");
        // Nothing left to redo.
        assert_eq!(
            cmd(&s, &cb, KeyPlatform::Other, "y", false),
            KeyHandleResult::Handled
        );
    }

    #[test]
    fn a_paste_and_a_delete_are_their_own_history_entries() {
        let cb = MemoryClipboard::new();
        let s = TextEditState::new();
        typed(&s, &cb, "ab");
        cb.write_text("XY");
        cmd(&s, &cb, KeyPlatform::Mac, "v", false);
        typed(&s, &cb, "c");
        key(&s, &cb, KeyPlatform::Mac, VK_BACK, false, false, false);
        assert_eq!(s.value(), "abXY");
        let mut seen = Vec::new();
        while s.undo() {
            seen.push(s.value());
        }
        assert_eq!(seen, ["abXYc", "abXY", "ab", ""]);
        // A new edit after undo drops the redo branch.
        typed(&s, &cb, "q");
        assert!(!s.redo());
    }

    #[test]
    fn caret_movement_closes_the_typing_group() {
        let cb = MemoryClipboard::new();
        let s = TextEditState::new();
        typed(&s, &cb, "ab");
        key(&s, &cb, KeyPlatform::Other, VK_LEFT, false, false, false);
        typed(&s, &cb, "c");
        assert_eq!(s.value(), "acb");
        assert!(s.undo());
        assert_eq!(s.value(), "ab");
        assert!(s.undo());
        assert_eq!(s.value(), "");
    }

    #[test]
    fn history_is_bounded() {
        let s = TextEditState::new();
        for _ in 0..(UNDO_LIMIT + 20) {
            s.paste_text("x");
        }
        let mut undos = 0;
        while s.undo() {
            undos += 1;
        }
        assert_eq!(undos, UNDO_LIMIT);
        assert_eq!(s.value(), "x".repeat(20));
    }

    #[test]
    fn a_paste_past_maxlength_is_truncated() {
        let cb = MemoryClipboard::new();
        let s = TextEditState::with_value("abc");
        s.set_max_length(Some(5));
        cb.write_text("12345");
        assert_eq!(
            cmd(&s, &cb, KeyPlatform::Mac, "v", false),
            KeyHandleResult::ValueChanged
        );
        assert_eq!(s.value(), "abc12");
        // Full: a paste inserts nothing and changes nothing.
        assert_eq!(
            cmd(&s, &cb, KeyPlatform::Mac, "v", false),
            KeyHandleResult::Handled
        );
        assert_eq!(s.value(), "abc12");
        // Replacing a selection frees its length; truncation keeps whole chars.
        s.set_selection(0, 3);
        cb.write_text("éèêë");
        cmd(&s, &cb, KeyPlatform::Mac, "v", false);
        assert_eq!(s.value(), "éèê12");
    }

    #[test]
    fn a_single_line_paste_drops_line_breaks_and_a_textarea_keeps_them() {
        let cb = MemoryClipboard::new();
        cb.write_text("a\r\nb\nc");
        let s = TextEditState::new();
        cmd(&s, &cb, KeyPlatform::Mac, "v", false);
        assert_eq!(s.value(), "abc");
        let t = TextAreaState::new();
        assert_eq!(t.edit.kind(), TextControlKind::MultiLine);
        handle_key(
            &t.edit,
            Some(&t),
            &cb,
            KeyPlatform::Mac,
            0,
            "v",
            true,
            false,
            false,
        );
        assert_eq!(t.value(), "a\nb\nc");
    }

    #[test]
    fn a_password_field_refuses_copy_and_cut_but_takes_a_paste() {
        let cb = MemoryClipboard::new();
        cb.write_text("before");
        let s = TextEditState::with_value("secret");
        s.set_kind(TextControlKind::Password);
        s.select_all();
        cmd(&s, &cb, KeyPlatform::Other, "c", false);
        cmd(&s, &cb, KeyPlatform::Other, "x", false);
        assert_eq!(cb.read_text().as_deref(), Some("before"));
        assert_eq!(s.value(), "secret");
        cmd(&s, &cb, KeyPlatform::Other, "v", false);
        assert_eq!(s.value(), "before");
    }

    #[test]
    fn a_read_only_field_copies_but_does_not_cut_paste_or_undo() {
        let cb = MemoryClipboard::new();
        let s = TextEditState::with_value("fixed");
        s.set_read_only(true);
        s.select_all();
        assert_eq!(
            cmd(&s, &cb, KeyPlatform::Mac, "c", false),
            KeyHandleResult::Handled
        );
        assert_eq!(cb.read_text().as_deref(), Some("fixed"));
        cmd(&s, &cb, KeyPlatform::Mac, "x", false);
        cmd(&s, &cb, KeyPlatform::Mac, "v", false);
        assert_eq!(s.value(), "fixed");
    }

    #[test]
    fn shift_left_twice_selects_two_chars_and_typing_replaces_them() {
        let cb = MemoryClipboard::new();
        let s = TextEditState::with_value("hello");
        key(&s, &cb, KeyPlatform::Mac, VK_LEFT, true, false, false);
        key(&s, &cb, KeyPlatform::Mac, VK_LEFT, true, false, false);
        assert_eq!(s.selected_text(), "lo");
        typed(&s, &cb, "p");
        assert_eq!(s.value(), "help");
        assert_eq!(s.caret_position(), 4);
    }

    #[test]
    fn shift_arrows_extend_from_the_anchor_both_ways() {
        let cb = MemoryClipboard::new();
        let s = TextEditState::with_value("hello");
        s.set_caret(2);
        key(&s, &cb, KeyPlatform::Other, VK_LEFT, true, false, false);
        key(&s, &cb, KeyPlatform::Other, VK_RIGHT, true, false, false);
        key(&s, &cb, KeyPlatform::Other, VK_RIGHT, true, false, false);
        assert_eq!(s.selection(), SelectionRange::new(2, 3));
        assert_eq!(s.selected_text(), "l");
        // Shift+Home from there selects back to the start from the anchor.
        key(&s, &cb, KeyPlatform::Other, VK_HOME, true, false, false);
        assert_eq!(s.selected_text(), "he");
        key(&s, &cb, KeyPlatform::Other, VK_END, true, false, false);
        assert_eq!(s.selected_text(), "llo");
        // An arrow without Shift collapses to that side.
        key(&s, &cb, KeyPlatform::Other, VK_LEFT, false, false, false);
        assert_eq!(s.selection(), SelectionRange::caret(2));
    }

    #[test]
    fn word_movement_uses_option_on_mac_and_ctrl_elsewhere() {
        let cb = MemoryClipboard::new();
        let s = TextEditState::with_value("one two, three");
        // macOS: Option+Left/Right by word; Cmd+Left/Right to the ends.
        key(&s, &cb, KeyPlatform::Mac, VK_LEFT, false, false, true);
        assert_eq!(s.caret_position(), 9);
        key(&s, &cb, KeyPlatform::Mac, VK_LEFT, false, false, true);
        assert_eq!(s.caret_position(), 4);
        key(&s, &cb, KeyPlatform::Mac, VK_RIGHT, false, false, true);
        assert_eq!(s.caret_position(), 7, "mac moves to the end of the word");
        key(&s, &cb, KeyPlatform::Mac, VK_LEFT, false, true, false);
        assert_eq!(s.caret_position(), 0);
        key(&s, &cb, KeyPlatform::Mac, VK_RIGHT, false, true, false);
        assert_eq!(s.caret_position(), 14);
        // Elsewhere: Ctrl+Left/Right by word; Right lands on the next word.
        s.set_caret(0);
        key(&s, &cb, KeyPlatform::Other, VK_RIGHT, false, true, false);
        assert_eq!(s.caret_position(), 4);
        key(&s, &cb, KeyPlatform::Other, VK_RIGHT, true, true, false);
        assert_eq!(s.selected_text(), "two, ");
        // Alt alone does nothing to the caret off macOS.
        s.set_caret(4);
        assert_eq!(
            key(&s, &cb, KeyPlatform::Other, VK_LEFT, false, false, true),
            KeyHandleResult::Unhandled
        );
        assert_eq!(s.caret_position(), 4);
    }

    #[test]
    fn word_movement_respects_multibyte_text() {
        let s = TextEditState::with_value("héllo wörld");
        s.move_word_left(false);
        assert_eq!(&s.value()[s.caret_position()..], "wörld");
        s.move_word_left(true);
        assert_eq!(s.selected_text(), "héllo ");
        s.move_left(false);
        s.move_right(false);
        assert_eq!(&s.value()[..s.caret_position()], "h");
        s.move_right(false);
        assert_eq!(&s.value()[..s.caret_position()], "hé");
    }

    #[test]
    fn delete_word_is_option_backspace_on_mac_and_ctrl_backspace_elsewhere() {
        let cb = MemoryClipboard::new();
        let s = TextEditState::with_value("one two three");
        assert_eq!(
            key(&s, &cb, KeyPlatform::Mac, VK_BACK, false, false, true),
            KeyHandleResult::ValueChanged
        );
        assert_eq!(s.value(), "one two ");
        assert_eq!(
            key(&s, &cb, KeyPlatform::Other, VK_BACK, false, true, false),
            KeyHandleResult::ValueChanged
        );
        assert_eq!(s.value(), "one ");
        s.set_caret(0);
        assert_eq!(
            key(&s, &cb, KeyPlatform::Other, VK_DELETE, false, true, false),
            KeyHandleResult::ValueChanged
        );
        assert_eq!(s.value(), " ");
        // Each deletion is its own undo step.
        assert!(s.undo());
        assert_eq!(s.value(), "one ");
        assert!(s.undo());
        assert_eq!(s.value(), "one two ");
    }

    #[test]
    fn the_command_modifier_never_types_its_letter() {
        let cb = MemoryClipboard::new();
        let s = TextEditState::with_value("ab");
        assert_eq!(
            cmd(&s, &cb, KeyPlatform::Mac, "q", false),
            KeyHandleResult::Unhandled
        );
        assert_eq!(s.value(), "ab");
    }

    #[test]
    fn chords_map_to_commands_per_platform() {
        let m = KeyPlatform::Mac;
        let o = KeyPlatform::Other;
        assert_eq!(
            command_for_key(m, 0, "c", true, false, false),
            Some(EditCommand::Copy)
        );
        assert_eq!(
            command_for_key(o, 0x43, "", true, false, false),
            Some(EditCommand::Copy)
        );
        assert_eq!(
            command_for_key(o, 0, "X", true, true, false),
            Some(EditCommand::Cut)
        );
        assert_eq!(
            command_for_key(m, 0, "v", true, false, false),
            Some(EditCommand::Paste)
        );
        assert_eq!(
            command_for_key(m, 0, "a", true, false, false),
            Some(EditCommand::SelectAll)
        );
        assert_eq!(
            command_for_key(m, 0, "z", true, false, false),
            Some(EditCommand::Undo)
        );
        assert_eq!(
            command_for_key(m, 0, "z", true, true, false),
            Some(EditCommand::Redo)
        );
        assert_eq!(
            command_for_key(o, 0, "y", true, false, false),
            Some(EditCommand::Redo)
        );
        assert_eq!(
            command_for_key(m, VK_BACK, "", false, false, true),
            Some(EditCommand::DeleteWordBackward)
        );
        assert_eq!(
            command_for_key(o, VK_DELETE, "", true, false, false),
            Some(EditCommand::DeleteWordForward)
        );
        assert_eq!(command_for_key(m, 0, "c", false, false, false), None);
        assert_eq!(command_for_key(o, VK_BACK, "", false, false, true), None);
    }

    #[test]
    fn exec_command_names_and_input_types() {
        assert_eq!(
            EditCommand::from_exec_name("selectAll", None),
            Some(EditCommand::SelectAll)
        );
        assert_eq!(
            EditCommand::from_exec_name("SELECTALL", None),
            Some(EditCommand::SelectAll)
        );
        assert_eq!(
            EditCommand::from_exec_name("insertText", Some("hi")),
            Some(EditCommand::InsertText("hi".into()))
        );
        assert_eq!(EditCommand::from_exec_name("bold", None), None);
        assert_eq!(EditCommand::Paste.input_type(), Some("insertFromPaste"));
        assert_eq!(EditCommand::Cut.input_type(), Some("deleteByCut"));
        assert_eq!(EditCommand::Undo.input_type(), Some("historyUndo"));
        assert_eq!(EditCommand::Redo.input_type(), Some("historyRedo"));
        assert_eq!(EditCommand::Copy.input_type(), None);
        assert!(EditCommand::Paste.mutates() && !EditCommand::Copy.mutates());
    }

    #[test]
    fn run_command_reports_the_input_event_to_fire() {
        let cb = MemoryClipboard::new();
        cb.write_text("zz");
        let s = TextEditState::with_value("ab");
        assert!(s.can_run(&EditCommand::Paste, &cb));
        assert!(!s.can_run(&EditCommand::Redo, &cb));
        let out = s.run_command(&EditCommand::Paste, &cb);
        assert!(out.handled && out.changed);
        assert_eq!(out.input_type, Some("insertFromPaste"));
        assert_eq!(out.data.as_deref(), Some("zz"));
        let out = s.run_command(&EditCommand::Undo, &cb);
        assert_eq!((out.changed, out.input_type), (true, Some("historyUndo")));
        assert_eq!(s.value(), "ab");
        let out = s.run_command(&EditCommand::Copy, &cb);
        assert!(out.handled && !out.changed);
    }

    #[test]
    fn setting_the_value_clears_the_history() {
        let s = TextEditState::new();
        s.paste_text("abc");
        s.set_value("script");
        assert!(!s.can_undo());
        assert_eq!(s.value(), "script");
    }
}
