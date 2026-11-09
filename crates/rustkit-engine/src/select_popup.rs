//! The drop-down of a `<select>` (CH2.E6): the list of options a click,
//! Space or Alt+Down on a closed select opens over the page, which WebKit
//! drew natively until HiWave's own chrome moved to RustKit.
//!
//! This module is the popup's model and paint, with no engine in it: the
//! options a select offers ([`SelectOption`], read from the DOM by
//! [`options_of`]), where the list goes ([`SelectPopup::open`]: under the
//! control, above it when there is no room below), which row is
//! highlighted and how keys, the pointer and the wheel move it, and the
//! display commands that draw it. The engine (`Engine::open_select_popup`
//! and friends in lib.rs) keeps one per view, appends [`SelectPopup::commands`]
//! after the page's display list (so a headless capture shows it and it
//! paints over everything), routes the pointer and keys to it while it is
//! open, and commits a pick through script (`selectedIndex`, `input`,
//! `change`).
//!
//! Geometry is in DOCUMENT coordinates, as the display list is.

use rustkit_css::Color;
use rustkit_dom::{Node, NodeType};
use rustkit_layout::{DisplayCommand, Rect};
use std::rc::Rc;

/// At most this many rows show at once; more scroll (Chromium's popup
/// shows 20).
pub(crate) const MAX_VISIBLE_ROWS: usize = 20;
/// Left inset of an option's text in its row, and the extra inset of an
/// option under an `<optgroup>` label.
const TEXT_INSET: f32 = 6.0;
const GROUP_INSET: f32 = 12.0;
/// Width kept for the scrollbar when the list scrolls.
const SCROLLBAR_WIDTH: f32 = 8.0;

const POPUP_BORDER: Color = Color { r: 118, g: 118, b: 118, a: 1.0 };
/// The highlighted row (pointer or keyboard), as Chromium's popup paints
/// it: the system highlight with white text.
const HIGHLIGHT_ROW: Color = Color { r: 30, g: 144, b: 255, a: 1.0 };
const HIGHLIGHT_TEXT: Color = Color { r: 255, g: 255, b: 255, a: 1.0 };
/// The selected option when the highlight is elsewhere.
const SELECTED_ROW: Color = Color { r: 206, g: 206, b: 206, a: 1.0 };
/// A disabled option or an option under a disabled group (CSS `GrayText`).
const DISABLED_TEXT: Color = Color { r: 109, g: 109, b: 109, a: 1.0 };
const SCROLL_THUMB: Color = Color { r: 160, g: 160, b: 160, a: 1.0 };

/// One option of a select, in tree order (HTML §4.10.7 "list of options":
/// the select's `<option>` children and the options of its `<optgroup>`
/// children).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SelectOption {
    /// The option's raw NodeId.
    pub node: usize,
    /// What the row shows: the `label` attribute, else the text with ASCII
    /// whitespace stripped and collapsed.
    pub label: String,
    /// Its own `disabled`, or its group's.
    pub disabled: bool,
    /// Whether it carries the `selected` attribute (its default
    /// selectedness, which script or the user may have changed since).
    pub default_selected: bool,
    /// The label of the `<optgroup>` it is in, if any.
    pub group: Option<String>,
}

/// The select's list of options (see [`SelectOption`]).
pub(crate) fn options_of(select: &Rc<Node>) -> Vec<SelectOption> {
    fn element(node: &Rc<Node>, tag: &str) -> bool {
        matches!(&node.node_type, NodeType::Element { tag_name, .. } if tag_name.eq_ignore_ascii_case(tag))
    }
    fn collapsed(text: &str) -> String {
        text.split(|c: char| matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' '))
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
    // Shown with its ASCII whitespace collapsed either way, as Chrome
    // draws a popup row.
    fn label_of(node: &Rc<Node>) -> String {
        match node.get_attribute("label") {
            Some(label) => collapsed(label),
            None => collapsed(&node.text_content()),
        }
    }
    let option = |node: &Rc<Node>, group: Option<(&String, bool)>| SelectOption {
        node: node.id.raw(),
        label: label_of(node),
        disabled: node.get_attribute("disabled").is_some() || group.is_some_and(|(_, off)| off),
        default_selected: node.get_attribute("selected").is_some(),
        group: group.map(|(label, _)| label.clone()),
    };
    let mut out = Vec::new();
    for child in select.children() {
        if element(&child, "option") {
            out.push(option(&child, None));
        } else if element(&child, "optgroup") {
            let label = child.get_attribute("label").unwrap_or_default().to_string();
            let off = child.get_attribute("disabled").is_some();
            for grandchild in child.children() {
                if element(&grandchild, "option") {
                    out.push(option(&grandchild, Some((&label, off))));
                }
            }
        }
    }
    out
}

/// The option a single select shows, given each option's selectedness:
/// the last selected one, else the first enabled one (HTML §4.10.7
/// selectedness setting algorithm), else none.
pub(crate) fn displayed_index(options: &[SelectOption], selected: impl Fn(&SelectOption) -> bool) -> Option<usize> {
    options
        .iter()
        .rposition(&selected)
        .or_else(|| options.iter().position(|o| !o.disabled))
}

/// The next enabled option from `from` in direction `step` (+1 / -1),
/// `from` itself excluded; with `from` `None`, the first (last) enabled
/// one. `None` when there is none that way: a closed select's arrow key
/// at the end of the list changes nothing, as in Chrome.
pub(crate) fn next_enabled(options: &[SelectOption], from: Option<usize>, step: isize) -> Option<usize> {
    let n = options.len() as isize;
    let mut i = match from {
        Some(i) => i as isize + step,
        None if step > 0 => 0,
        None => n - 1,
    };
    while (0..n).contains(&i) {
        if !options[i as usize].disabled {
            return Some(i as usize);
        }
        i += step;
    }
    None
}

/// The first or last enabled option.
pub(crate) fn edge_enabled(options: &[SelectOption], last: bool) -> Option<usize> {
    next_enabled(options, None, if last { -1 } else { 1 })
}

/// Type-to-select: the next enabled option after `from` (wrapping) whose
/// label starts with `prefix`, ignoring case. With a one-character prefix
/// the search starts after `from`, so the same letter cycles through the
/// options it starts; a longer prefix (typed quickly) may stay on `from`.
pub(crate) fn typeahead(options: &[SelectOption], from: Option<usize>, prefix: &str) -> Option<usize> {
    let prefix = prefix.to_lowercase();
    if prefix.is_empty() || options.is_empty() {
        return None;
    }
    let n = options.len();
    let cycle = prefix.chars().count() == 1;
    let start = match from {
        Some(i) if cycle => i + 1,
        Some(i) => i,
        None => 0,
    };
    (0..n)
        .map(|k| (start + k) % n)
        .find(|&i| !options[i].disabled && options[i].label.trim_start().to_lowercase().starts_with(&prefix))
}

/// The keys typed for type-to-select (HTML leaves it to the UA; this is
/// Chromium's): keys less than [`TYPEAHEAD_TIMEOUT`] apart make a prefix,
/// and one letter repeated cycles through the options it starts. An open
/// popup and a focused closed select search the same way.
#[derive(Debug, Clone, Default)]
pub(crate) struct TypeAhead {
    typed: String,
    at: Option<std::time::Instant>,
}

impl TypeAhead {
    /// Whether a prefix is being typed at `now` (a Space then extends it
    /// instead of picking).
    pub fn active(&self, now: std::time::Instant) -> bool {
        !self.typed.is_empty() && self.at.is_some_and(|at| now.duration_since(at) <= TYPEAHEAD_TIMEOUT)
    }

    /// `c` typed at `now`: the option it leads to from `from`.
    pub fn find(
        &mut self,
        options: &[SelectOption],
        from: Option<usize>,
        c: char,
        now: std::time::Instant,
    ) -> Option<usize> {
        if !self.active(now) {
            self.typed.clear();
        }
        self.at = Some(now);
        self.typed.push(c);
        let mut chars = self.typed.chars();
        let first = chars.next();
        let prefix = if chars.all(|k| Some(k) == first) { c.to_string() } else { self.typed.clone() };
        typeahead(options, from, &prefix)
    }
}

/// A row of the popup: a group label, or an option (its index in the list
/// of options).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Row {
    Group,
    Option(usize),
}

/// The text style of the rows: the select's own.
#[derive(Debug, Clone)]
pub(crate) struct PopupFont {
    pub size: f32,
    pub family: String,
    pub weight: u16,
    pub color: Color,
    /// The popup's background: the select's own `background-color` when it
    /// has one, else white (CH2.E6b).
    pub background: Color,
}

/// An open drop-down.
#[derive(Debug, Clone)]
pub(crate) struct SelectPopup {
    /// The select's raw NodeId.
    pub select: usize,
    pub options: Vec<SelectOption>,
    /// What each row shows, top to bottom (group labels interleaved).
    pub rows: Vec<(Row, String)>,
    /// The option selected when the popup opened.
    pub selected: Option<usize>,
    /// The option the pointer or the keys are on: what Enter picks.
    pub highlighted: Option<usize>,
    /// The popup's border box.
    pub rect: Rect,
    pub row_height: f32,
    /// The first row shown.
    pub first_row: usize,
    /// How many rows show at once.
    pub visible_rows: usize,
    pub font: PopupFont,
    /// What was typed for type-to-select.
    pub typed: TypeAhead,
}

/// How long a pause ends a type-to-select prefix (Chromium: 1 s).
const TYPEAHEAD_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1000);

impl SelectPopup {
    /// Open the popup of the select `select` whose border box is `anchor`,
    /// in a window showing `visible` (both in document coordinates).
    /// `text_width` measures a label in the select's font. Placed under
    /// the control when the list fits there, else above it when it fits
    /// there, else on the side with more room, shortened to fit.
    pub fn open(
        select: usize,
        options: Vec<SelectOption>,
        selected: Option<usize>,
        anchor: Rect,
        visible: Rect,
        font: PopupFont,
        text_width: impl Fn(&str) -> f32,
    ) -> Self {
        let mut rows = Vec::new();
        let mut group: Option<&String> = None;
        for (i, o) in options.iter().enumerate() {
            if o.group.as_ref() != group {
                group = o.group.as_ref();
                if let Some(label) = group {
                    rows.push((Row::Group, label.clone()));
                }
            }
            rows.push((Row::Option(i), o.label.clone()));
        }
        let row_height = (font.size * 1.2 + 4.0).round().max(1.0);
        let border = 1.0;
        let below = (visible.y + visible.height) - (anchor.y + anchor.height);
        let above = anchor.y - visible.y;
        let fits = |room: f32| ((room - 2.0 * border) / row_height).floor().max(0.0) as usize;
        let want = rows.len().clamp(1, MAX_VISIBLE_ROWS);
        let (visible_rows, down) = if fits(below) >= want {
            (want, true)
        } else if fits(above) >= want {
            (want, false)
        } else if below >= above {
            (fits(below).clamp(1, want), true)
        } else {
            (fits(above).clamp(1, want), false)
        };
        let scrolls = visible_rows < rows.len();
        let widest = rows
            .iter()
            .map(|(row, label)| {
                let inset = match row {
                    Row::Option(i) if options[*i].group.is_some() => TEXT_INSET + GROUP_INSET,
                    _ => TEXT_INSET,
                };
                text_width(label) + 2.0 * inset
            })
            .fold(0.0f32, f32::max);
        let width = anchor
            .width
            .max(widest + 2.0 * border + if scrolls { SCROLLBAR_WIDTH } else { 0.0 })
            .min(visible.width.max(anchor.width));
        let height = visible_rows as f32 * row_height + 2.0 * border;
        // Kept inside the window horizontally where it can be.
        let right_edge = visible.x + visible.width;
        let x = if anchor.x + width > right_edge {
            (right_edge - width).max(visible.x)
        } else {
            anchor.x
        };
        let y = if down { anchor.y + anchor.height } else { anchor.y - height };
        let mut popup = SelectPopup {
            select,
            options,
            rows,
            selected,
            highlighted: selected,
            rect: Rect::new(x, y, width, height),
            row_height,
            first_row: 0,
            visible_rows,
            font,
            typed: TypeAhead::default(),
        };
        popup.scroll_to_highlight();
        popup
    }

    /// The row an option is on.
    fn row_of(&self, option: usize) -> Option<usize> {
        self.rows.iter().position(|(r, _)| *r == Row::Option(option))
    }

    /// The rows' area: the popup inside its border.
    fn inner(&self) -> Rect {
        Rect::new(self.rect.x + 1.0, self.rect.y + 1.0, self.rect.width - 2.0, self.rect.height - 2.0)
    }

    /// Whether a document point is on the popup.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.rect.x && x < self.rect.x + self.rect.width && y >= self.rect.y && y < self.rect.y + self.rect.height
    }

    /// The option under a document point, enabled or not; `None` off the
    /// rows or on a group label.
    pub fn option_at(&self, x: f32, y: f32) -> Option<usize> {
        let inner = self.inner();
        if x < inner.x || x >= inner.x + inner.width || y < inner.y || y >= inner.y + inner.height {
            return None;
        }
        let row = self.first_row + ((y - inner.y) / self.row_height) as usize;
        match self.rows.get(row)?.0 {
            Row::Option(i) => Some(i),
            Row::Group => None,
        }
    }

    /// The pointer moved over the popup: an enabled option under it is
    /// highlighted. Returns true when the highlight moved.
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        match self.option_at(x, y) {
            Some(i) if !self.options[i].disabled && self.highlighted != Some(i) => {
                self.highlighted = Some(i);
                true
            }
            _ => false,
        }
    }

    /// Move the highlight to `option`, scrolled into view.
    pub fn highlight(&mut self, option: Option<usize>) -> bool {
        if option.is_none() || option == self.highlighted {
            return false;
        }
        self.highlighted = option;
        self.scroll_to_highlight();
        true
    }

    /// The highlight `step` enabled options on (Arrow keys: ±1; Page keys:
    /// a page of rows), stopping at the ends.
    pub fn step(&mut self, step: isize) -> bool {
        let mut at = self.highlighted;
        for _ in 0..step.unsigned_abs().max(1) {
            match next_enabled(&self.options, at, step.signum()) {
                Some(next) => at = Some(next),
                None => break,
            }
        }
        self.highlight(at)
    }

    /// A typed character for type-to-select at `now`.
    pub fn type_char(&mut self, c: char, now: std::time::Instant) -> bool {
        let found = self.typed.find(&self.options, self.highlighted, c, now);
        self.highlight(found)
    }

    /// Scroll the rows by `rows` (wheel), within the list.
    pub fn scroll_rows(&mut self, rows: isize) -> bool {
        let max = self.rows.len().saturating_sub(self.visible_rows);
        let first = (self.first_row as isize + rows).clamp(0, max as isize) as usize;
        let moved = first != self.first_row;
        self.first_row = first;
        moved
    }

    /// Scroll so the highlighted row shows, and its group label with it
    /// when it is the group's first option.
    fn scroll_to_highlight(&mut self) {
        let Some(row) = self.highlighted.and_then(|i| self.row_of(i)) else {
            return;
        };
        let top = if row > 0 && self.rows[row - 1].0 == Row::Group { row - 1 } else { row };
        if top < self.first_row {
            self.first_row = top;
        } else if row >= self.first_row + self.visible_rows {
            self.first_row = row + 1 - self.visible_rows;
        }
    }

    /// The display commands that draw the popup, over everything before
    /// them: a stacking context of its own at the top z-index, the frame,
    /// the visible rows clipped to it, and a scroll thumb when it scrolls.
    pub fn commands(&self) -> Vec<DisplayCommand> {
        let mut out = vec![
            DisplayCommand::PushStackingContext { z_index: i32::MAX, rect: self.rect },
            DisplayCommand::BoxShadow {
                offset_x: 0.0,
                offset_y: 2.0,
                blur_radius: 6.0,
                spread_radius: 0.0,
                color: Color::new(0, 0, 0, 0.25),
                rect: self.rect,
                border_radius: Default::default(),
                inset: false,
            },
            DisplayCommand::FillRect { rect: self.rect, color: self.font.background },
            DisplayCommand::Border {
                color: POPUP_BORDER,
                rect: self.rect,
                top: 1.0,
                right: 1.0,
                bottom: 1.0,
                left: 1.0,
            },
        ];
        let inner = self.inner();
        out.push(DisplayCommand::PushClip(inner));
        let scrolls = self.visible_rows < self.rows.len();
        let row_width = inner.width - if scrolls { SCROLLBAR_WIDTH } else { 0.0 };
        for (k, (row, label)) in self.rows.iter().enumerate().skip(self.first_row).take(self.visible_rows) {
            let y = inner.y + (k - self.first_row) as f32 * self.row_height;
            let row_rect = Rect::new(inner.x, y, row_width, self.row_height);
            let (inset, weight, color) = match *row {
                Row::Group => (TEXT_INSET, 700, self.font.color),
                Row::Option(i) => {
                    let o = &self.options[i];
                    let inset = if o.group.is_some() { TEXT_INSET + GROUP_INSET } else { TEXT_INSET };
                    let color = if self.highlighted == Some(i) {
                        out.push(DisplayCommand::FillRect { rect: row_rect, color: HIGHLIGHT_ROW });
                        HIGHLIGHT_TEXT
                    } else {
                        if self.selected == Some(i) {
                            out.push(DisplayCommand::FillRect { rect: row_rect, color: SELECTED_ROW });
                        }
                        if o.disabled {
                            DISABLED_TEXT
                        } else {
                            self.font.color
                        }
                    };
                    (inset, self.font.weight, color)
                }
            };
            if label.is_empty() {
                continue;
            }
            out.push(DisplayCommand::Text {
                text: label.clone(),
                x: inner.x + inset,
                y: y + ((self.row_height - self.font.size * 1.2) / 2.0).max(0.0),
                color,
                font_size: self.font.size,
                font_family: self.font.family.clone(),
                font_weight: weight,
                font_style: 0,
                advances: None,
                ascent: None,
                run: None,
                faces: None,
            });
        }
        if scrolls {
            let track = inner.height;
            let total = self.rows.len() as f32;
            let thumb_h = (track * self.visible_rows as f32 / total).max(12.0).min(track);
            let max_first = (self.rows.len() - self.visible_rows) as f32;
            let thumb_y = inner.y + (track - thumb_h) * (self.first_row as f32 / max_first);
            out.push(DisplayCommand::FillRect {
                rect: Rect::new(inner.x + inner.width - SCROLLBAR_WIDTH + 2.0, thumb_y, SCROLLBAR_WIDTH - 4.0, thumb_h),
                color: SCROLL_THUMB,
            });
        }
        out.push(DisplayCommand::PopClip);
        out.push(DisplayCommand::PopStackingContext);
        out
    }
}

#[cfg(test)]
#[path = "select_popup_tests.rs"]
mod tests;
