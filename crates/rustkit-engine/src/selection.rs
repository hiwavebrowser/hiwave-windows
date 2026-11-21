//! The document selection and mouse selection (CH2.E2).
//!
//! One selection per view: an anchor and a focus, each a DOM node and an
//! offset (UTF-16 code units in a text node, a child index in any other
//! node), the same boundary points script sees through `getSelection()`.
//! The page's `Selection` (rustkit-bindings, dom.rs) and the engine's copy
//! here are kept in step: a drag moves both (`set_doc_selection`), and a
//! script's change reaches the engine when script settles
//! (`follow_script_selection`).
//!
//! Painting: `selection_paint` turns the selection into the selected
//! characters of each text node, in the white-space-collapsed characters
//! the layout's text runs are made of, and the display list paints them
//! behind the glyphs (`DisplayList::build_with_selection`). A selection
//! change builds the list again over the same boxes; nothing is laid out.
//!
//! Text controls keep their own selection in their edit state
//! (`TextEditState`); a press or drag in a focused `<input>` or
//! `<textarea>` moves that one instead.

use super::*;
use rustkit_layout::collapse_map;

/// A boundary point: a node and an offset in it (DOM §5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DocPoint {
    pub node: usize,
    pub offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DocSelection {
    pub anchor: DocPoint,
    pub focus: DocPoint,
}

/// What a drag with the button held moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectDrag {
    /// The document selection, from this anchor.
    Text { anchor: DocPoint },
    /// A text control's selection, from this character.
    Control { node: usize, anchor: usize },
}

/// The UTF-16 offset of character `index` of `text`.
fn utf16_of_char(text: &str, index: usize) -> usize {
    text.chars().take(index).map(char::len_utf16).sum()
}

/// The character at UTF-16 offset `offset` of `text` (a split surrogate
/// pair rounds down).
fn char_of_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.chars().enumerate() {
        if units + c.len_utf16() > offset {
            return i;
        }
        units += c.len_utf16();
    }
    text.chars().count()
}

/// The byte offset of character `index` of `text` (its length past the
/// end). `TextEditState` offsets are bytes on character boundaries; the
/// mouse and the painter count characters.
pub(crate) fn byte_of_char(text: &str, index: usize) -> usize {
    text.char_indices().nth(index).map_or(text.len(), |(b, _)| b)
}

/// The character at byte offset `offset` of `text` (a byte inside a
/// character rounds down to it).
pub(crate) fn char_of_byte(text: &str, offset: usize) -> usize {
    text.char_indices().take_while(|(b, _)| *b < offset.min(text.len())).count()
        - usize::from(offset < text.len() && !text.is_char_boundary(offset))
}

fn text_of(node: &Node) -> Option<&str> {
    match &node.node_type {
        NodeType::Text(t) => Some(t.as_str()),
        _ => None,
    }
}

/// Where a word starts and ends around character `index` of `text`: a run
/// of word characters (letters, digits, `_`), a run of white space, or one
/// other character.
pub(crate) fn word_around(text: &str, index: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let at = index.min(chars.len() - 1);
    let class = |c: char| -> u8 {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let k = class(chars[at]);
    if k == 2 {
        return (at, at + 1);
    }
    let mut start = at;
    while start > 0 && class(chars[start - 1]) == k {
        start -= 1;
    }
    let mut end = at + 1;
    while end < chars.len() && class(chars[end]) == k {
        end += 1;
    }
    (start, end)
}

/// Elements whose box is inline in the UA sheet: a triple-click selects
/// the nearest ancestor that is none of these.
const INLINE_TAGS: &[&str] = &[
    "a", "abbr", "b", "bdi", "bdo", "cite", "code", "data", "dfn", "em", "font", "i", "kbd", "label", "mark", "q",
    "s", "samp", "small", "span", "strong", "sub", "sup", "time", "u", "var", "wbr",
];

/// Tree order of every node under `root`: its preorder number and the last
/// preorder number in its subtree.
fn tree_order(root: &Rc<Node>) -> HashMap<usize, (usize, usize)> {
    fn walk(node: &Rc<Node>, next: &mut usize, out: &mut HashMap<usize, (usize, usize)>) {
        let me = *next;
        *next += 1;
        for child in node.children() {
            walk(&child, next, out);
        }
        out.insert(node.id.raw(), (me, *next - 1));
    }
    let mut out = HashMap::new();
    walk(root, &mut 0, &mut out);
    out
}

/// A boundary point as a key that sorts in tree order: the preorder number
/// of the node it is in front of (or of the text node it is in) and a
/// character offset in that text node.
fn point_key(doc: &Document, order: &HashMap<usize, (usize, usize)>, p: DocPoint) -> Option<(usize, usize)> {
    let node = doc.get_node(rustkit_dom::NodeId::new(p.node))?;
    let &(me, last) = order.get(&p.node)?;
    if let Some(text) = text_of(&node) {
        return Some((me, char_of_utf16(text, p.offset)));
    }
    match node.children().get(p.offset) {
        Some(child) => Some((order.get(&child.id.raw())?.0, 0)),
        None => Some((last + 1, 0)),
    }
}

/// The view's collapsible flag per text node, from the runs' sources.
fn collapsible_by_node(tree: &LayoutBox, out: &mut HashMap<usize, bool>) {
    for src in &tree.text_sources {
        out.insert(src.node, src.collapsible);
    }
    for child in &tree.children {
        collapsible_by_node(child, out);
    }
}

impl Engine {
    /// The view's document selection as `(anchor node, anchor offset, focus
    /// node, focus offset)`, offsets as script sees them.
    pub fn document_selection(&self, id: EngineViewId) -> Option<(usize, usize, usize, usize)> {
        let sel = self.views.get(&id)?.doc_selection?;
        Some((sel.anchor.node, sel.anchor.offset, sel.focus.node, sel.focus.offset))
    }

    /// The selected characters of each text node, as the display list
    /// paints them (see the module comment). Empty without a selection.
    pub(crate) fn selection_paint(&self, id: EngineViewId, root: &LayoutBox) -> HashMap<usize, (usize, usize)> {
        let mut out = HashMap::new();
        let Some(view) = self.views.get(&id) else {
            return out;
        };
        let (Some(sel), Some(doc)) = (view.doc_selection, view.document.as_ref()) else {
            return out;
        };
        if sel.anchor == sel.focus {
            return out;
        }
        let order = tree_order(doc.root());
        let (Some(a), Some(f)) = (point_key(doc, &order, sel.anchor), point_key(doc, &order, sel.focus)) else {
            return out;
        };
        let (start, end) = if f < a { (f, a) } else { (a, f) };
        let mut collapsible = HashMap::new();
        collapsible_by_node(root, &mut collapsible);
        for (node, flag) in collapsible {
            let Some(&(me, _)) = order.get(&node) else {
                continue;
            };
            if me < start.0 || me > end.0 {
                continue;
            }
            let Some(dom) = doc.get_node(rustkit_dom::NodeId::new(node)) else {
                continue;
            };
            let Some(text) = text_of(&dom) else {
                continue;
            };
            let len = text.chars().count();
            let from = if me == start.0 { start.1.min(len) } else { 0 };
            let to = if me == end.0 { end.1.min(len) } else { len };
            if from >= to {
                continue;
            }
            let map = collapse_map(text, flag);
            if map[from] < map[to] {
                out.insert(node, (map[from], map[to]));
            }
        }
        out
    }

    /// The text position under (or nearest to) a VIEWPORT point.
    fn text_point_at(&self, id: EngineViewId, viewport_x: f32, viewport_y: f32) -> Option<DocPoint> {
        let view = self.views.get(&id)?;
        let layout = view.layout.as_ref()?;
        let hit = layout.text_position_at(viewport_x + view.scroll_offset.0, viewport_y + view.scroll_offset.1)?;
        let node = view.document.as_ref()?.get_node(rustkit_dom::NodeId::new(hit.node))?;
        let text = text_of(&node)?;
        // The first DOM character that collapses to the hit's position.
        let map = collapse_map(text, hit.collapsible);
        let index = map.iter().position(|&m| m >= hit.offset).unwrap_or(map.len() - 1);
        Some(DocPoint { node: hit.node, offset: utf16_of_char(text, index) })
    }

    /// Move the document selection (`None` clears it): the page's
    /// `getSelection()` follows and hears `selectionchange`, and the view
    /// paints it.
    pub(crate) fn set_doc_selection(&mut self, id: EngineViewId, sel: Option<DocSelection>) {
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        if view.doc_selection == sel {
            return;
        }
        view.doc_selection = sel;
        if let Some(bindings) = view.bindings.as_ref() {
            let ends = sel.map(|s| (s.anchor.node, s.anchor.offset, s.focus.node, s.focus.offset));
            if let Err(e) = bindings.set_selection(ends) {
                debug!(?id, error = %e, "selectionchange listener threw");
            }
        }
        self.repaint_selection(id);
    }

    /// Build the display list again over the same boxes, for a selection
    /// that moved.
    pub(crate) fn repaint_selection(&mut self, id: EngineViewId) {
        if self.views.get(&id).is_some_and(|v| v.layout.is_some()) {
            self.relist_after_element_scroll(id);
            if let Err(e) = self.render(id) {
                debug!(?id, error = %e, "render after a selection change failed");
            }
        }
    }

    /// Take a selection script moved since the engine last looked. Returns
    /// true when it changed what is painted.
    pub(crate) fn follow_script_selection(&mut self, id: EngineViewId) -> bool {
        let Some(moved) = self
            .views
            .get(&id)
            .and_then(|v| v.bindings.as_ref())
            .and_then(|b| b.take_selection_change())
        else {
            return false;
        };
        let sel = moved.map(|(a, ao, f, fo)| DocSelection {
            anchor: DocPoint { node: a, offset: ao },
            focus: DocPoint { node: f, offset: fo },
        });
        let Some(view) = self.views.get_mut(&id) else {
            return false;
        };
        let changed = view.doc_selection != sel;
        view.doc_selection = sel;
        changed
    }

    /// The focused text control (raw id), when it is an `<input>` or
    /// `<textarea>` with an edit state.
    pub(crate) fn focused_text_control(&self, id: EngineViewId) -> Option<usize> {
        let view = self.views.get(&id)?;
        let raw = view.lay.focused_node.get()?.raw();
        view.lay.edit_states.borrow().contains_key(&raw).then_some(raw)
    }

    /// The character of the focused text control's value nearest to a
    /// VIEWPORT point: lines at `\n`, characters measured in the control's
    /// font from the start of its content box.
    fn control_char_at(&self, id: EngineViewId, raw: usize, viewport_x: f32, viewport_y: f32) -> Option<usize> {
        let view = self.views.get(&id)?;
        let tree = view.layout.as_ref()?;
        let control = box_of_node(tree, raw)?;
        let value = view.lay.edit_states.borrow().get(&raw)?.value();
        // A password field paints a bullet per character.
        let password = matches!(
            &control.box_type,
            rustkit_layout::BoxType::FormControl(rustkit_layout::FormControlType::TextInput { input_type, .. })
                if input_type.eq_ignore_ascii_case("password")
        );
        let value = if password { value.chars().map(|_| rustkit_layout::PASSWORD_MASK).collect() } else { value };
        let style = &control.style;
        let font_size = match style.font_size {
            rustkit_css::Length::Px(px) => px,
            _ => 13.333,
        };
        let content = control.dimensions.content;
        let (x, y) = (viewport_x + view.scroll_offset.0, viewport_y + view.scroll_offset.1);
        let line_height = font_size * 1.2;
        let lines: Vec<&str> = value.split('\n').collect();
        let line = if lines.len() > 1 {
            (((y - content.y) / line_height).floor().max(0.0) as usize).min(lines.len() - 1)
        } else {
            0
        };
        let before: usize = lines[..line].iter().map(|l| l.chars().count() + 1).sum();
        let mut left = content.x;
        let mut index = 0;
        for c in lines[line].chars() {
            let adv = rustkit_layout::measure_text_advanced(
                &c.to_string(),
                &style.font_family,
                font_size,
                style.font_weight,
                style.font_style,
            )
            .width;
            if x < left + adv / 2.0 {
                break;
            }
            left += adv;
            index += 1;
        }
        Some(before + index)
    }

    /// The focused text control, and how many selections script has set,
    /// read on each side of a press's focus change.
    pub(crate) fn press_focus_marks(&self, id: EngineViewId) -> (Option<usize>, u64) {
        let count = self
            .views
            .get(&id)
            .and_then(|v| v.bindings.as_ref())
            .map_or(0, |b| b.script_selections());
        (self.focused_text_control(id), count)
    }

    /// Paint the selection a focus handler set during a press.
    pub(crate) fn relayout_after_press_focus(&mut self, id: EngineViewId) {
        if let Err(e) = self.relayout_for(id, RelayoutCause::HoverOrFocus) {
            debug!(?id, error = %e, "relayout after a focus handler's selection failed");
        }
    }

    /// The press's default action for the selection (after the focus has
    /// moved): in a focused text control, place the caret, select a word
    /// (`clicks == 2`) or everything (3), or extend (`shift`); elsewhere
    /// the same for the document selection, a triple-click selecting the
    /// paragraph. A drag from here moves what the press set.
    pub(crate) fn press_selects(&mut self, id: EngineViewId, viewport_x: f32, viewport_y: f32, clicks: u32, shift: bool) {
        let hit = self.element_at_point(id, viewport_x, viewport_y);
        if let Some(raw) = self.focused_text_control(id).filter(|raw| hit == Some(*raw)) {
            let Some(at) = self.control_char_at(id, raw, viewport_x, viewport_y) else {
                return;
            };
            let Some(view) = self.views.get_mut(&id) else {
                return;
            };
            let anchor = {
                let states = view.lay.edit_states.borrow();
                let Some(state) = states.get(&raw) else {
                    return;
                };
                let value = state.value();
                // Characters here; the edit state takes bytes.
                let sel = state.selection().normalize();
                let (sel_start, sel_end) = (char_of_byte(&value, sel.start), char_of_byte(&value, sel.end));
                let (anchor, start, end) = match clicks {
                    2 => {
                        let (s, e) = word_around(&value, at);
                        (s, s, e)
                    }
                    n if n >= 3 => (0, 0, value.chars().count()),
                    _ if shift => {
                        let anchor = if at < sel_start { sel_end } else { sel_start };
                        (anchor, anchor.min(at), anchor.max(at))
                    }
                    _ => (at, at, at),
                };
                state.set_selection(byte_of_char(&value, start), byte_of_char(&value, end));
                anchor
            };
            view.select_drag = Some(SelectDrag::Control { node: raw, anchor });
            view.doc_selection = None;
            if let Some(bindings) = view.bindings.as_ref() {
                let _ = bindings.set_selection(None);
            }
            if let Err(e) = self.relayout_for(id, RelayoutCause::HoverOrFocus) {
                debug!(?id, error = %e, "relayout after a press in a text control failed");
            }
            return;
        }
        // In a contenteditable host the caret stays in the host: a press
        // nearest to text outside it puts the caret at the host's end.
        let host = hit.and_then(|raw| self.node(id, raw)).and_then(|n| Self::editing_host_of(&n));
        let point = self.text_point_at(id, viewport_x, viewport_y);
        let point = match &host {
            Some((host, _)) => {
                let inside = point.and_then(|p| self.node(id, p.node)).is_some_and(|n| {
                    let mut cur = Some(n);
                    while let Some(c) = cur {
                        if c.id.raw() == *host {
                            return true;
                        }
                        cur = c.parent();
                    }
                    false
                });
                if inside {
                    point
                } else {
                    self.node(id, *host).map(|h| DocPoint { node: *host, offset: h.children().len() })
                }
            }
            None => point,
        };
        let Some(point) = point else {
            if let Some(view) = self.views.get_mut(&id) {
                view.select_drag = None;
            }
            self.set_doc_selection(id, None);
            return;
        };
        let sel = match clicks {
            2 => self.word_selection(id, point),
            n if n >= 3 => self.paragraph_selection(id, point),
            _ => {
                let anchor = match self.views.get(&id).and_then(|v| v.doc_selection) {
                    Some(old) if shift => old.anchor,
                    _ => point,
                };
                Some(DocSelection { anchor, focus: point })
            }
        };
        if let Some(view) = self.views.get_mut(&id) {
            view.select_drag = sel.map(|s| SelectDrag::Text { anchor: s.anchor });
        }
        self.set_doc_selection(id, sel);
    }

    /// The word around a text position.
    fn word_selection(&self, id: EngineViewId, point: DocPoint) -> Option<DocSelection> {
        let node = self.node(id, point.node)?;
        let text = text_of(&node)?;
        let (s, e) = word_around(text, char_of_utf16(text, point.offset));
        Some(DocSelection {
            anchor: DocPoint { node: point.node, offset: utf16_of_char(text, s) },
            focus: DocPoint { node: point.node, offset: utf16_of_char(text, e) },
        })
    }

    /// All of the block a text position is in.
    fn paragraph_selection(&self, id: EngineViewId, point: DocPoint) -> Option<DocSelection> {
        let mut block = self.node(id, point.node)?.parent()?;
        loop {
            let inline = match &block.node_type {
                NodeType::Element { tag_name, .. } => INLINE_TAGS.contains(&tag_name.to_ascii_lowercase().as_str()),
                _ => false,
            };
            if !inline {
                break;
            }
            match block.parent() {
                Some(p) => block = p,
                None => break,
            }
        }
        let raw = block.id.raw();
        Some(DocSelection {
            anchor: DocPoint { node: raw, offset: 0 },
            focus: DocPoint { node: raw, offset: block.children().len() },
        })
    }

    /// The pointer moved with the button held: the selection the press
    /// started follows it.
    pub(crate) fn drag_selects(&mut self, id: EngineViewId, viewport_x: f32, viewport_y: f32) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        if !view.primary_button_down {
            return;
        }
        match view.select_drag {
            Some(SelectDrag::Text { anchor }) => {
                if let Some(focus) = self.text_point_at(id, viewport_x, viewport_y) {
                    self.set_doc_selection(id, Some(DocSelection { anchor, focus }));
                }
            }
            Some(SelectDrag::Control { node, anchor }) => {
                let Some(at) = self.control_char_at(id, node, viewport_x, viewport_y) else {
                    return;
                };
                let moved = {
                    let states = view.lay.edit_states.borrow();
                    let Some(state) = states.get(&node) else {
                        return;
                    };
                    let value = state.value();
                    let (s, e) = (byte_of_char(&value, anchor.min(at)), byte_of_char(&value, anchor.max(at)));
                    let before = state.selection().normalize();
                    state.set_selection(s, e);
                    (before.start, before.end) != (s, e)
                };
                if moved {
                    if let Err(e) = self.relayout_for(id, RelayoutCause::HoverOrFocus) {
                        debug!(?id, error = %e, "relayout after a drag in a text control failed");
                    }
                }
            }
            None => {}
        }
    }
}

/// Whether an element's `contenteditable` makes it editable (`""`, `true`,
/// `plaintext-only`), refuses (`false`), or says nothing (inherit).
fn editable_attr(node: &Node) -> Option<bool> {
    let value = node.get_attribute("contenteditable")?;
    Some(matches!(value.to_ascii_lowercase().as_str(), "" | "true" | "plaintext-only"))
}

impl Engine {
    /// The editing host `node` is in (HTML §6.8.1): the outermost element
    /// of its run of editable ancestors, as `(raw id, tag)`. `None` outside
    /// one, or under `contenteditable=false`.
    pub(crate) fn editing_host_of(node: &Rc<Node>) -> Option<(usize, String)> {
        let mut host = None;
        let mut cur = Some(node.clone());
        while let Some(n) = cur {
            if let NodeType::Element { tag_name, .. } = &n.node_type {
                match editable_attr(&n) {
                    Some(true) => host = Some((n.id.raw(), tag_name.to_lowercase())),
                    Some(false) => break,
                    None if host.is_some() => break,
                    None => {}
                }
            }
            cur = n.parent();
        }
        host
    }

    /// Whether the node is a contenteditable host.
    pub(crate) fn is_editing_host(&self, id: EngineViewId, raw: usize) -> bool {
        self.node(id, raw)
            .and_then(|n| Self::editing_host_of(&n))
            .is_some_and(|(host, _)| host == raw)
    }

    /// An editing key in the focused contenteditable host: the bindings
    /// edit the DOM at the selection and fire `input`
    /// (`DomBindings::edit_key`), and what they wrote is laid out. Returns
    /// whether anything changed.
    pub(crate) fn editable_key(&mut self, id: EngineViewId, host: usize, key_code: u32, key: &str, ctrl: bool) -> bool {
        let op = match key_code {
            0x08 => "back",
            0x2E => "del",
            0x0D => "enter",
            0x25 => "left",
            0x27 => "right",
            0x41 if ctrl => "all",
            _ if !ctrl && !key.is_empty() && !key.chars().any(char::is_control) => "insert",
            _ => return false,
        };
        let Some(bindings) = self.views.get(&id).and_then(|v| v.bindings.as_ref()) else {
            return false;
        };
        let _turn = rustkit_js::DiagTurn::enter("live");
        let changed = match bindings.edit_key(host, op, key) {
            Ok(changed) => changed,
            Err(e) => {
                debug!(?id, error = %e, "editing key threw");
                false
            }
        };
        if let Err(e) = self.flush_script_dom_writes(id, Some(RelayoutCause::Input)) {
            debug!(?id, error = %e, "relayout after an editing key failed");
        }
        changed
    }

    /// Where the caret of a focused contenteditable host is painted: a
    /// text node and a collapsed character in it (see `selection_paint`),
    /// or, in a host with no text there, the host's own box.
    pub(crate) fn caret_paint(&self, id: EngineViewId, root: &LayoutBox) -> Option<CaretPaint> {
        let view = self.views.get(&id)?;
        let focused = view.lay.focused_node.get()?.raw();
        if !self.is_editing_host(id, focused) {
            return None;
        }
        let sel = view.doc_selection?;
        if sel.anchor != sel.focus {
            return None;
        }
        let doc = view.document.as_ref()?;
        let node = doc.get_node(rustkit_dom::NodeId::new(sel.focus.node))?;
        let in_host = {
            let mut cur = Some(node.clone());
            let mut found = false;
            while let Some(n) = cur {
                if n.id.raw() == focused {
                    found = true;
                    break;
                }
                cur = n.parent();
            }
            found
        };
        if !in_host {
            return None;
        }
        let mut collapsible = HashMap::new();
        collapsible_by_node(root, &mut collapsible);
        // A point between nodes: the end of the text before it, or the
        // start of the text after it.
        let (text_node, char_offset) = match text_of(&node) {
            Some(text) => (node.clone(), char_of_utf16(text, sel.focus.offset)),
            None => {
                let kids = node.children();
                let before = sel.focus.offset.checked_sub(1).and_then(|i| kids.get(i)).filter(|k| text_of(k).is_some());
                let after = kids.get(sel.focus.offset).filter(|k| text_of(k).is_some());
                match (before, after) {
                    (Some(b), _) => (b.clone(), text_of(b).map_or(0, |t| t.chars().count())),
                    (None, Some(a)) => (a.clone(), 0),
                    _ => return Some(CaretPaint::Box(focused)),
                }
            }
        };
        let raw = text_node.id.raw();
        let Some(&flag) = collapsible.get(&raw) else {
            return Some(CaretPaint::Box(focused));
        };
        let map = collapse_map(text_of(&text_node)?, flag);
        Some(CaretPaint::Text(raw, map[char_offset.min(map.len() - 1)]))
    }
}

/// See [`Engine::caret_paint`].
pub(crate) enum CaretPaint {
    Text(usize, usize),
    Box(usize),
}
