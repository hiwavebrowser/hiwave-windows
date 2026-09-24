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
#[derive(Debug)]
#[allow(dead_code)]
pub struct TextEditState {
    /// The text value.
    value: RefCell<String>,
    /// Current selection.
    selection: Cell<SelectionRange>,
    /// Selection direction.
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
}

impl Default for TextEditState {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEditState {
    /// Create a new empty text edit state.
    pub fn new() -> Self {
        Self {
            value: RefCell::new(String::new()),
            selection: Cell::new(SelectionRange::caret(0)),
            selection_direction: Cell::new(SelectionDirection::None),
            max_length: Cell::new(None),
            min_length: Cell::new(0),
            read_only: Cell::new(false),
            disabled: Cell::new(false),
            placeholder: RefCell::new(String::new()),
            pattern: RefCell::new(None),
            required: Cell::new(false),
        }
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
        }
    }

    /// Get the current value.
    pub fn value(&self) -> String {
        self.value.borrow().clone()
    }

    /// Set the value (resets selection to end).
    pub fn set_value(&self, value: impl Into<String>) {
        let text = value.into();
        let len = text.len();
        *self.value.borrow_mut() = text;
        self.selection.set(SelectionRange::caret(len));
    }

    /// Get the current selection.
    pub fn selection(&self) -> SelectionRange {
        self.selection.get()
    }

    /// Set the selection range.
    pub fn set_selection(&self, start: usize, end: usize) {
        let len = self.value.borrow().len();
        let start = start.min(len);
        let end = end.min(len);
        self.selection.set(SelectionRange::new(start, end));
    }

    /// Get the caret position (selection start).
    pub fn caret_position(&self) -> usize {
        self.selection.get().start
    }

    /// Set the caret position.
    pub fn set_caret(&self, position: usize) {
        let len = self.value.borrow().len();
        let pos = position.min(len);
        self.selection.set(SelectionRange::caret(pos));
    }

    /// Get selected text.
    pub fn selected_text(&self) -> String {
        let sel = self.selection.get().normalize();
        let value = self.value.borrow();
        if sel.start < value.len() && sel.end <= value.len() {
            value[sel.start..sel.end].to_string()
        } else {
            String::new()
        }
    }

    /// Select all text.
    pub fn select_all(&self) {
        let len = self.value.borrow().len();
        self.selection.set(SelectionRange::new(0, len));
    }

    /// Check if the input can be edited.
    pub fn is_editable(&self) -> bool {
        !self.read_only.get() && !self.disabled.get()
    }

    /// Insert text at the current caret position (or replace selection).
    pub fn insert_text(&self, text: &str) -> bool {
        if !self.is_editable() {
            return false;
        }

        let sel = self.selection.get().normalize();
        let mut value = self.value.borrow_mut();

        // Check max length
        if let Some(max) = self.max_length.get() {
            let new_len = value.len() - sel.len() + text.len();
            if new_len > max {
                return false;
            }
        }

        // Replace selection with new text
        let start = sel.start.min(value.len());
        let end = sel.end.min(value.len());

        value.replace_range(start..end, text);

        // Move caret to end of inserted text
        let new_pos = start + text.len();
        drop(value);
        self.selection.set(SelectionRange::caret(new_pos));

        true
    }

    /// Delete the selection or character before caret (backspace).
    pub fn delete_backward(&self) -> bool {
        if !self.is_editable() {
            return false;
        }

        let sel = self.selection.get().normalize();
        let mut value = self.value.borrow_mut();

        if !sel.is_collapsed() {
            // Delete selection
            let start = sel.start.min(value.len());
            let end = sel.end.min(value.len());
            value.replace_range(start..end, "");
            drop(value);
            self.selection.set(SelectionRange::caret(start));
        } else if sel.start > 0 {
            // Delete character before caret
            let pos = sel.start;
            let start = pos.saturating_sub(1);
            value.replace_range(start..pos, "");
            drop(value);
            self.selection.set(SelectionRange::caret(start));
        } else {
            return false;
        }

        true
    }

    /// Delete the selection or character after caret (delete key).
    pub fn delete_forward(&self) -> bool {
        if !self.is_editable() {
            return false;
        }

        let sel = self.selection.get().normalize();
        let mut value = self.value.borrow_mut();
        let len = value.len();

        if !sel.is_collapsed() {
            // Delete selection
            let start = sel.start.min(len);
            let end = sel.end.min(len);
            value.replace_range(start..end, "");
            drop(value);
            self.selection.set(SelectionRange::caret(start));
        } else if sel.start < len {
            // Delete character after caret
            let pos = sel.start;
            let end = (pos + 1).min(len);
            value.replace_range(pos..end, "");
            drop(value);
            // Caret stays in place
        } else {
            return false;
        }

        true
    }

    /// Move caret left.
    pub fn move_left(&self, extend_selection: bool) {
        let sel = self.selection.get();
        let new_pos = sel.start.saturating_sub(1);

        if extend_selection {
            self.selection.set(SelectionRange::new(new_pos, sel.end));
        } else if sel.is_collapsed() {
            self.selection.set(SelectionRange::caret(new_pos));
        } else {
            // Collapse to start
            self.selection
                .set(SelectionRange::caret(sel.normalize().start));
        }
    }

    /// Move caret right.
    pub fn move_right(&self, extend_selection: bool) {
        let sel = self.selection.get();
        let len = self.value.borrow().len();
        let new_pos = (sel.end + 1).min(len);

        if extend_selection {
            self.selection.set(SelectionRange::new(sel.start, new_pos));
        } else if sel.is_collapsed() {
            self.selection.set(SelectionRange::caret(new_pos));
        } else {
            // Collapse to end
            self.selection
                .set(SelectionRange::caret(sel.normalize().end));
        }
    }

    /// Move caret to start.
    pub fn move_to_start(&self, extend_selection: bool) {
        let sel = self.selection.get();
        if extend_selection {
            self.selection.set(SelectionRange::new(0, sel.end));
        } else {
            self.selection.set(SelectionRange::caret(0));
        }
    }

    /// Move caret to end.
    pub fn move_to_end(&self, extend_selection: bool) {
        let sel = self.selection.get();
        let len = self.value.borrow().len();
        if extend_selection {
            self.selection.set(SelectionRange::new(sel.start, len));
        } else {
            self.selection.set(SelectionRange::caret(len));
        }
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
        Self {
            edit: TextEditState::new(),
            rows: Cell::new(2),
            cols: Cell::new(20),
            wrap: Cell::new(WrapMode::Soft),
        }
    }

    /// Create with initial value.
    pub fn with_value(value: impl Into<String>) -> Self {
        Self {
            edit: TextEditState::with_value(value),
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
        self.offset_to_position(self.edit.caret_position())
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

        if extend_selection {
            let sel = self.edit.selection();
            self.edit.set_selection(new_offset, sel.end);
        } else {
            self.edit.set_caret(new_offset);
        }
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

        if extend_selection {
            let sel = self.edit.selection();
            self.edit.set_selection(sel.start, new_offset);
        } else {
            self.edit.set_caret(new_offset);
        }
    }

    /// Move caret to start of current line.
    pub fn move_to_line_start(&self, extend_selection: bool) {
        let pos = self.caret_line_position();
        let new_offset = self.position_to_offset(LinePosition {
            line: pos.line,
            column: 0,
        });

        if extend_selection {
            let sel = self.edit.selection();
            self.edit.set_selection(new_offset, sel.end);
        } else {
            self.edit.set_caret(new_offset);
        }
    }

    /// Move caret to end of current line.
    pub fn move_to_line_end(&self, extend_selection: bool) {
        let pos = self.caret_line_position();
        let line_len = self.line_length(pos.line);
        let new_offset = self.position_to_offset(LinePosition {
            line: pos.line,
            column: line_len,
        });

        if extend_selection {
            let sel = self.edit.selection();
            self.edit.set_selection(sel.start, new_offset);
        } else {
            self.edit.set_caret(new_offset);
        }
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
        } else if action.starts_with("http://") || action.starts_with("https://") {
            action
        } else if action.starts_with('/') {
            // Absolute path
            if let Some(origin) = extract_origin(base_url) {
                format!("{}{}", origin, action)
            } else {
                action
            }
        } else {
            // Relative path
            if let Some(base) = base_url.rfind('/') {
                format!("{}/{}", &base_url[..base], action)
            } else {
                action
            }
        };

        match method {
            FormMethod::Get => {
                // Append data to URL as query string
                let query = String::from_utf8_lossy(&FormState::encode_url(entries)).to_string();
                let final_url = if url.contains('?') {
                    format!("{}&{}", url, query)
                } else {
                    format!("{}?{}", url, query)
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

/// Extract the origin (scheme + host + port) from a URL.
fn extract_origin(url: &str) -> Option<String> {
    if let Some(idx) = url.find("://") {
        let rest = &url[idx + 3..];
        if let Some(path_idx) = rest.find('/') {
            return Some(format!("{}{}", &url[..idx + 3], &rest[..path_idx]));
        }
        return Some(url.to_string());
    }
    None
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

    /// Handle a key event for a single-line text input.
    /// Returns what action was taken.
    pub fn handle_input_key(
        state: &TextEditState,
        key_code: u32,
        key: &str,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> KeyHandleResult {
        // Don't handle if Alt is pressed (usually browser shortcuts)
        if alt {
            return KeyHandleResult::Unhandled;
        }

        // Handle keyboard shortcuts (Ctrl+key)
        if ctrl {
            return handle_ctrl_key(state, key_code, key);
        }

        // Handle navigation and editing keys
        match key_code {
            // Arrow keys
            0x25 => {
                // Left
                state.move_left(shift);
                KeyHandleResult::SelectionChanged
            }
            0x27 => {
                // Right
                state.move_right(shift);
                KeyHandleResult::SelectionChanged
            }
            0x24 => {
                // Home
                state.move_to_start(shift);
                KeyHandleResult::SelectionChanged
            }
            0x23 => {
                // End
                state.move_to_end(shift);
                KeyHandleResult::SelectionChanged
            }

            // Editing keys
            0x08 => {
                // Backspace
                if state.delete_backward() {
                    KeyHandleResult::ValueChanged
                } else {
                    KeyHandleResult::Handled
                }
            }
            0x2E => {
                // Delete
                if state.delete_forward() {
                    KeyHandleResult::ValueChanged
                } else {
                    KeyHandleResult::Handled
                }
            }
            0x0D => {
                // Enter - submit form for single-line input
                KeyHandleResult::Submit
            }
            0x09 => {
                // Tab - move focus
                KeyHandleResult::Unhandled
            }
            0x1B => {
                // Escape - typically blur
                KeyHandleResult::Unhandled
            }

            // Character input
            _ => {
                if !key.is_empty() && key.chars().count() == 1 {
                    // SAFETY: Checked above that string is non-empty and has exactly 1 char
                    let ch = key.chars().next().unwrap();
                    // Filter out control characters
                    if ch.is_control() {
                        KeyHandleResult::Unhandled
                    } else if state.insert_text(key) {
                        KeyHandleResult::ValueChanged
                    } else {
                        KeyHandleResult::Handled
                    }
                } else {
                    KeyHandleResult::Unhandled
                }
            }
        }
    }

    /// Handle a key event for a multi-line textarea.
    pub fn handle_textarea_key(
        state: &TextAreaState,
        key_code: u32,
        key: &str,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> KeyHandleResult {
        if alt {
            return KeyHandleResult::Unhandled;
        }

        if ctrl {
            return handle_ctrl_key(&state.edit, key_code, key);
        }

        match key_code {
            // Arrow keys
            0x25 => {
                // Left
                state.edit.move_left(shift);
                KeyHandleResult::SelectionChanged
            }
            0x27 => {
                // Right
                state.edit.move_right(shift);
                KeyHandleResult::SelectionChanged
            }
            0x26 => {
                // Up - move up a line
                state.move_up(shift);
                KeyHandleResult::SelectionChanged
            }
            0x28 => {
                // Down - move down a line
                state.move_down(shift);
                KeyHandleResult::SelectionChanged
            }
            0x24 => {
                // Home
                if ctrl {
                    state.edit.move_to_start(shift);
                } else {
                    state.move_to_line_start(shift);
                }
                KeyHandleResult::SelectionChanged
            }
            0x23 => {
                // End
                if ctrl {
                    state.edit.move_to_end(shift);
                } else {
                    state.move_to_line_end(shift);
                }
                KeyHandleResult::SelectionChanged
            }

            // Editing keys
            0x08 => {
                // Backspace
                if state.edit.delete_backward() {
                    KeyHandleResult::ValueChanged
                } else {
                    KeyHandleResult::Handled
                }
            }
            0x2E => {
                // Delete
                if state.edit.delete_forward() {
                    KeyHandleResult::ValueChanged
                } else {
                    KeyHandleResult::Handled
                }
            }
            0x0D => {
                // Enter - insert newline in textarea
                if state.insert_newline() {
                    KeyHandleResult::ValueChanged
                } else {
                    KeyHandleResult::Handled
                }
            }
            0x09 => {
                // Tab - could insert tab or move focus
                KeyHandleResult::Unhandled
            }
            0x1B => {
                // Escape
                KeyHandleResult::Unhandled
            }

            // Character input
            _ => {
                if !key.is_empty() && key.chars().count() == 1 {
                    // SAFETY: Checked above that string is non-empty and has exactly 1 char
                    let ch = key.chars().next().unwrap();
                    if ch.is_control() {
                        KeyHandleResult::Unhandled
                    } else if state.edit.insert_text(key) {
                        KeyHandleResult::ValueChanged
                    } else {
                        KeyHandleResult::Handled
                    }
                } else {
                    KeyHandleResult::Unhandled
                }
            }
        }
    }

    /// Handle Ctrl+key shortcuts.
    fn handle_ctrl_key(state: &TextEditState, key_code: u32, _key: &str) -> KeyHandleResult {
        match key_code {
            // Ctrl+A - Select all
            0x41 => {
                state.select_all();
                KeyHandleResult::SelectionChanged
            }
            // Ctrl+C - Copy (handled by OS/clipboard API)
            0x43 => KeyHandleResult::Unhandled,
            // Ctrl+V - Paste (handled by OS/clipboard API)
            0x56 => KeyHandleResult::Unhandled,
            // Ctrl+X - Cut (handled by OS/clipboard API)
            0x58 => KeyHandleResult::Unhandled,
            // Ctrl+Z - Undo (not implemented yet)
            0x5A => KeyHandleResult::Unhandled,
            // Ctrl+Y - Redo (not implemented yet)
            0x59 => KeyHandleResult::Unhandled,
            // Ctrl+Home - Go to start
            0x24 => {
                state.move_to_start(false);
                KeyHandleResult::SelectionChanged
            }
            // Ctrl+End - Go to end
            0x23 => {
                state.move_to_end(false);
                KeyHandleResult::SelectionChanged
            }
            // Ctrl+Left - Move word left (simplified: move to start)
            0x25 => {
                move_word_left(state);
                KeyHandleResult::SelectionChanged
            }
            // Ctrl+Right - Move word right (simplified: move to end)
            0x27 => {
                move_word_right(state);
                KeyHandleResult::SelectionChanged
            }
            // Ctrl+Backspace - Delete word backward
            0x08 => {
                if delete_word_backward(state) {
                    KeyHandleResult::ValueChanged
                } else {
                    KeyHandleResult::Handled
                }
            }
            // Ctrl+Delete - Delete word forward
            0x2E => {
                if delete_word_forward(state) {
                    KeyHandleResult::ValueChanged
                } else {
                    KeyHandleResult::Handled
                }
            }
            _ => KeyHandleResult::Unhandled,
        }
    }

    /// Move caret to the start of the previous word.
    fn move_word_left(state: &TextEditState) {
        let value = state.value();
        let pos = state.caret_position();
        if pos == 0 {
            return;
        }

        let chars: Vec<char> = value.chars().collect();
        let mut new_pos = pos.saturating_sub(1);

        // Skip whitespace
        while new_pos > 0 && chars.get(new_pos).is_some_and(|c| c.is_whitespace()) {
            new_pos -= 1;
        }

        // Skip word characters
        while new_pos > 0 && chars.get(new_pos - 1).is_some_and(|c| !c.is_whitespace()) {
            new_pos -= 1;
        }

        state.set_caret(new_pos);
    }

    /// Move caret to the end of the next word.
    fn move_word_right(state: &TextEditState) {
        let value = state.value();
        let pos = state.caret_position();
        let len = value.chars().count();
        if pos >= len {
            return;
        }

        let chars: Vec<char> = value.chars().collect();
        let mut new_pos = pos;

        // Skip current word
        while new_pos < len && chars.get(new_pos).is_some_and(|c| !c.is_whitespace()) {
            new_pos += 1;
        }

        // Skip whitespace
        while new_pos < len && chars.get(new_pos).is_some_and(|c| c.is_whitespace()) {
            new_pos += 1;
        }

        state.set_caret(new_pos);
    }

    /// Delete the word before the caret.
    fn delete_word_backward(state: &TextEditState) -> bool {
        if !state.is_editable() {
            return false;
        }

        let sel = state.selection();
        if !sel.is_collapsed() {
            return state.delete_backward();
        }

        let value = state.value();
        let pos = state.caret_position();
        if pos == 0 {
            return false;
        }

        let chars: Vec<char> = value.chars().collect();
        let mut start = pos.saturating_sub(1);

        // Skip whitespace
        while start > 0 && chars.get(start).is_some_and(|c| c.is_whitespace()) {
            start -= 1;
        }

        // Skip word characters
        while start > 0 && chars.get(start - 1).is_some_and(|c| !c.is_whitespace()) {
            start -= 1;
        }

        // Select and delete
        state.set_selection(start, pos);
        state.delete_backward()
    }

    /// Delete the word after the caret.
    fn delete_word_forward(state: &TextEditState) -> bool {
        if !state.is_editable() {
            return false;
        }

        let sel = state.selection();
        if !sel.is_collapsed() {
            return state.delete_forward();
        }

        let value = state.value();
        let pos = state.caret_position();
        let len = value.chars().count();
        if pos >= len {
            return false;
        }

        let chars: Vec<char> = value.chars().collect();
        let mut end = pos;

        // Skip word characters
        while end < len && chars.get(end).is_some_and(|c| !c.is_whitespace()) {
            end += 1;
        }

        // Skip whitespace
        while end < len && chars.get(end).is_some_and(|c| c.is_whitespace()) {
            end += 1;
        }

        // Select and delete
        state.set_selection(pos, end);
        state.delete_forward()
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

    #[test]
    fn test_extract_origin() {
        assert_eq!(
            extract_origin("https://example.com/path/page"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            extract_origin("http://localhost:8080/"),
            Some("http://localhost:8080".to_string())
        );
        assert_eq!(extract_origin("invalid"), None);
    }
}
