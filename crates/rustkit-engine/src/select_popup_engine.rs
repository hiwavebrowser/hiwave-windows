//! The engine side of the `<select>` drop-down (CH2.E6; the model and its
//! paint are `select_popup`).
//!
//! - A press on a focusable drop-down select (no `size` above 1, no
//!   `multiple`, not disabled) whose `mousedown` went uncancelled, or
//!   Space, Alt+Down/Alt+Up or F4 while one is focused, opens its popup at
//!   the control, as Chrome does. Releasing that press on an option picks
//!   it (press, drag, release); released anywhere else, the popup stays.
//! - The popup closes when its select leaves the document, loses the focus
//!   or is disabled (checked after each relayout), and when the view is
//!   resized.
//! - While it is open the popup owns the pointer, the wheel and the keys:
//!   the page hears none of them. A move over it highlights the option under
//!   the pointer; a click on an enabled option picks it; a press anywhere
//!   else closes it (and its release goes nowhere). Arrow, Page, Home and
//!   End keys move the highlight, a letter jumps to the next option starting
//!   with it, Enter, Space or Tab pick the highlighted one, Escape closes.
//! - While a drop-down is focused and closed, arrow keys, Home, End and
//!   letters change its value directly, as in Chrome on Windows and Linux.
//! - A pick goes through script (`DomBindings::pick_option`): selectedness,
//!   then trusted `input` and `change`. The option's selectedness reaches
//!   layout and `:checked` through the checkedness side table.
//!
//! The popup's commands follow the page's in the view's display list, so
//! a headless capture shows it and it paints over everything.

use super::*;
use crate::select_popup::{edge_enabled, next_enabled, PopupFont, SelectOption, SelectPopup, TypeAhead};

/// Win32 virtual keys, as `handle_text_key` takes them.
const VK_TAB: u32 = 0x09;
const VK_ENTER: u32 = 0x0D;
const VK_ESCAPE: u32 = 0x1B;
const VK_SPACE: u32 = 0x20;
const VK_PAGE_UP: u32 = 0x21;
const VK_PAGE_DOWN: u32 = 0x22;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_F4: u32 = 0x73;

/// A key as the popup reads it: the virtual key, else the named key in
/// `key` (`"ArrowDown"`), else the typed character.
fn key_of(key_code: u32, key: &str) -> (u32, Option<char>) {
    if key_code != 0 {
        return (key_code, None);
    }
    let named = match key {
        "Tab" => VK_TAB,
        "Enter" => VK_ENTER,
        "Escape" => VK_ESCAPE,
        " " | "Spacebar" => VK_SPACE,
        "PageUp" => VK_PAGE_UP,
        "PageDown" => VK_PAGE_DOWN,
        "End" => VK_END,
        "Home" => VK_HOME,
        "ArrowLeft" => VK_LEFT,
        "ArrowUp" => VK_UP,
        "ArrowRight" => VK_RIGHT,
        "ArrowDown" => VK_DOWN,
        "F4" => VK_F4,
        _ => 0,
    };
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if named == 0 && !c.is_control() => (0, Some(c)),
        _ => (named, None),
    }
}

/// The border box of the first box `node` generated, where the window
/// shows it at the page scroll `window_scroll`: moved by the scroll of
/// every element scroller above it, and by the page's scroll unless a
/// `position: fixed` box (here or above) holds it to the window, whose
/// layout coordinates are already the window's. Transforms are not
/// applied.
fn viewport_rect_of(root: &LayoutBox, node: usize, window_scroll: (f32, f32)) -> Option<(&LayoutBox, Rect)> {
    fn walk(b: &LayoutBox, node: usize, scrolled: (f32, f32), fixed: bool) -> Option<(&LayoutBox, Rect, bool)> {
        // A fixed box is held to the window: no scroller above it moves it.
        let scrolled = if b.position == Position::Fixed { (0.0, 0.0) } else { scrolled };
        let fixed = fixed || b.position == Position::Fixed;
        if b.node_id == Some(node) {
            let r = b.dimensions.border_box();
            return Some((b, Rect::new(r.x - scrolled.0, r.y - scrolled.1, r.width, r.height), fixed));
        }
        // A scroller's own box stays put; what it holds moves.
        let inner = if b.is_element_scroller() {
            let (x, y) = b.element_scroll.get();
            (scrolled.0 + x, scrolled.1 + y)
        } else {
            scrolled
        };
        b.children.iter().find_map(|c| walk(c, node, inner, fixed))
    }
    let (b, r, fixed) = walk(root, node, (0.0, 0.0), false)?;
    Some(if fixed { (b, r) } else { (b, Rect::new(r.x - window_scroll.0, r.y - window_scroll.1, r.width, r.height)) })
}

impl Engine {
    /// Whether a drop-down is open in the view.
    pub fn select_popup_open(&self, id: EngineViewId) -> bool {
        self.views.get(&id).is_some_and(|v| v.lay.select_popup.borrow().is_some())
    }

    /// The `<select>` element `raw` as a drop-down that can open: a
    /// single select shown as a menu list (no `multiple`, `size` 1 or
    /// absent), not disabled.
    pub(crate) fn drop_down_select(&self, id: EngineViewId, raw: usize) -> Option<Rc<Node>> {
        let node = self.node(id, raw)?;
        let is_select = node.tag_name().is_some_and(|t| t.eq_ignore_ascii_case("select"));
        let size = node.get_attribute("size").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0);
        let ok = is_select
            && node.get_attribute("multiple").is_none()
            && size <= 1
            && !Self::in_disabled_control(node.clone());
        ok.then_some(node)
    }

    /// The select's options and the one it shows now.
    fn select_state(&self, id: EngineViewId, select: &Rc<Node>) -> (Vec<SelectOption>, Option<usize>) {
        let options = select_popup::options_of(select);
        let shown = self.views.get(&id).and_then(|view| {
            let live = view.lay.checked_states.borrow();
            select_popup::displayed_index(&options, |o| live.get(&o.node).copied().unwrap_or(o.default_selected))
        });
        (options, shown)
    }

    /// Open the drop-down of the select `raw`. Returns false when it is no
    /// drop-down, has no box or no options.
    pub(crate) fn open_select_popup(&mut self, id: EngineViewId, raw: usize) -> bool {
        let Some(select) = self.drop_down_select(id, raw) else {
            return false;
        };
        let (options, shown) = self.select_state(id, &select);
        if options.is_empty() {
            return false;
        }
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        let Some(layout) = view.layout.as_ref() else {
            return false;
        };
        // Placed where the window shows the control, then written in
        // document coordinates (the display list's, and what the pointer
        // routing above adds the page scroll back to).
        let Some((b, on_screen)) = viewport_rect_of(layout, raw, view.scroll_offset) else {
            return false;
        };
        let anchor = Rect::new(
            on_screen.x + view.scroll_offset.0,
            on_screen.y + view.scroll_offset.1,
            on_screen.width,
            on_screen.height,
        );
        let (w, h) = view
            .laid_with
            .as_ref()
            .map(|with| (with.size.0 as f32, with.size.1 as f32))
            .unwrap_or((layout.dimensions.border_box().width, anchor.y + anchor.height + 400.0));
        let visible = Rect::new(view.scroll_offset.0, view.scroll_offset.1, w, h);
        let font = PopupFont {
            size: match b.style.font_size {
                rustkit_css::Length::Px(px) => px,
                _ => 13.333,
            },
            family: b.style.font_family.clone(),
            weight: b.style.font_weight.0,
            color: if b.style.color.a > 0.0 { b.style.color } else { rustkit_css::Color::BLACK },
            // Chromium's drawn popup lists the options on the select's
            // background colour, in the select's colour (CH2.E6b).
            background: if b.style.background_color.a > 0.0 {
                rustkit_css::Color { a: 1.0, ..b.style.background_color }
            } else {
                rustkit_css::Color::WHITE
            },
        };
        let (family, size, weight) = (font.family.clone(), font.size, b.style.font_weight);
        let measure = |s: &str| {
            rustkit_layout::measure_text_advanced(s, &family, size, weight, rustkit_css::FontStyle::Normal).width
        };
        let popup = SelectPopup::open(raw, options, shown, anchor, visible, font, measure);
        debug!(?id, select = raw, rows = popup.rows.len(), "Opened a select popup");
        *view.lay.select_popup.borrow_mut() = Some(popup);
        self.repaint_select_popup(id);
        true
    }

    /// Close the drop-down, if one is open, without picking.
    pub fn close_select_popup(&mut self, id: EngineViewId) -> bool {
        let closed = self
            .views
            .get(&id)
            .and_then(|v| {
                v.lay.select_popup_opening_press.set(false);
                v.lay.select_popup.borrow_mut().take()
            })
            .is_some();
        if closed {
            self.repaint_select_popup(id);
        }
        closed
    }

    /// Close the drop-down and pick the highlighted option.
    fn commit_select_popup(&mut self, id: EngineViewId) {
        let popup = self.views.get(&id).and_then(|v| {
            v.lay.select_popup_opening_press.set(false);
            v.lay.select_popup.borrow_mut().take()
        });
        let Some(popup) = popup else {
            return;
        };
        self.repaint_select_popup(id);
        if let Some(index) = popup.highlighted {
            self.pick_select_option(id, popup.select, index);
        }
    }

    /// The user picked option `index` of the select `select`: its
    /// selectedness moves there, the select hears `input` and `change`,
    /// and what that changed is laid out. Nothing happens for a disabled
    /// option or the one already selected. Returns whether it changed.
    pub(crate) fn pick_select_option(&mut self, id: EngineViewId, select: usize, index: usize) -> bool {
        let Some(node) = self.node(id, select) else {
            return false;
        };
        let (options, shown) = self.select_state(id, &node);
        let Some(option) = options.get(index) else {
            return false;
        };
        if option.disabled || shown == Some(index) {
            return false;
        }
        let picked = match self.views.get(&id).and_then(|v| v.bindings.as_ref()) {
            Some(bindings) => {
                let _turn = rustkit_js::DiagTurn::enter("live");
                let armed = arm_live_turn(&self.config, bindings);
                let picked = bindings.pick_option(select, index).unwrap_or_else(|e| {
                    debug!(?id, error = %e, "select change listener threw");
                    true
                });
                if let Some(stopped) = end_live_turn(&self.config, armed, bindings, "event:change") {
                    if let Some(view) = self.views.get_mut(&id) {
                        view.script_log.push(stopped);
                    }
                }
                picked
            }
            // No script: the side table alone, which layout and `:checked`
            // read.
            None => {
                if let Some(view) = self.views.get(&id) {
                    let mut live = view.lay.checked_states.borrow_mut();
                    for (i, o) in options.iter().enumerate() {
                        live.insert(o.node, i == index);
                    }
                }
                if let Err(e) = self.relayout_for(id, RelayoutCause::Input) {
                    debug!(?id, error = %e, "relayout after a select pick failed");
                }
                true
            }
        };
        if let Err(e) = self.flush_script_dom_writes(id, Some(RelayoutCause::Input)) {
            debug!(?id, error = %e, "relayout after a select pick failed");
        }
        picked
    }

    /// Put the popup's commands at the end of the view's display list (or
    /// take them out when it closed) and present the frame. The page is
    /// not laid out again.
    pub(crate) fn repaint_select_popup(&mut self, id: EngineViewId) {
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        let Some(list) = view.display_list.as_ref() else {
            return;
        };
        let mut list = DisplayList::clone(list);
        let painted = view.lay.select_popup_painted.get();
        list.commands.truncate(list.commands.len().saturating_sub(painted));
        let commands = view.lay.select_popup.borrow().as_ref().map(SelectPopup::commands).unwrap_or_default();
        view.lay.select_popup_painted.set(commands.len());
        list.commands.extend(commands);
        view.display_list = Some(Arc::new(list));
        view.frame_generation += 1;
        if let Err(e) = self.render(id) {
            debug!(?id, error = %e, "render of the select popup failed");
        }
    }

    /// Append the popup's commands to a display list just built from a
    /// layout (none when no popup is open), and note how many there are
    /// for the next repaint to take off.
    pub(crate) fn append_select_popup(&self, id: EngineViewId, list: &mut DisplayList) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let commands = view.lay.select_popup.borrow().as_ref().map(SelectPopup::commands).unwrap_or_default();
        view.lay.select_popup_painted.set(commands.len());
        list.commands.extend(commands);
    }

    /// A press at VIEWPORT coordinates while a drop-down is open: the
    /// popup's. `Some(true)` when it took the press (inside it, or outside,
    /// which closes it), `None` when no popup is open.
    pub(crate) fn select_popup_press(&mut self, id: EngineViewId, viewport_x: f32, viewport_y: f32) -> Option<bool> {
        let view = self.views.get(&id)?;
        let (x, y) = (viewport_x + view.scroll_offset.0, viewport_y + view.scroll_offset.1);
        let inside = view.lay.select_popup.borrow().as_ref()?.contains(x, y);
        if !inside {
            // Chrome closes the popup at the press, and the press (and its
            // release) does nothing else: not even reopen a select clicked
            // to close it.
            view.lay.select_popup_release.set(true);
            self.close_select_popup(id);
        }
        Some(true)
    }

    /// A release at VIEWPORT coordinates while a drop-down is open, or
    /// after a press closed one: the popup's. `None` when the page should
    /// have it.
    pub(crate) fn select_popup_release(&mut self, id: EngineViewId, viewport_x: f32, viewport_y: f32) -> Option<()> {
        let view = self.views.get_mut(&id)?;
        if view.lay.select_popup_release.replace(false) {
            view.primary_button_down = false;
            view.press_target = None;
            return Some(());
        }
        let (x, y) = (viewport_x + view.scroll_offset.0, viewport_y + view.scroll_offset.1);
        let hit = {
            let popup = view.lay.select_popup.borrow();
            let popup = popup.as_ref()?;
            if popup.contains(x, y) {
                Some(popup.option_at(x, y).filter(|&i| !popup.options[i].disabled))
            } else {
                None
            }
        };
        // The release of the press that opened the popup: on an enabled
        // option it picks it (a drag from the select); anywhere else it is
        // the page's (`mouseup`, `click` at the select) and the popup stays.
        if view.lay.select_popup_opening_press.replace(false) {
            let Some(Some(index)) = hit else {
                return None;
            };
            view.primary_button_down = false;
            view.press_target = None;
            view.compat_mouse_suppressed = false;
            let changed = std::mem::take(&mut *view.lay.active_chain.borrow_mut());
            view.pointer_restyle |= Self::chain_change_restyles(view, |reads| &reads.active, &changed);
            if let Some(popup) = view.lay.select_popup.borrow_mut().as_mut() {
                popup.highlighted = Some(index);
            }
            self.commit_select_popup(id);
            self.settle_pointer_restyle(id);
            return Some(());
        }
        view.primary_button_down = false;
        view.press_target = None;
        match hit {
            // On an enabled option: it is picked.
            Some(Some(index)) => {
                if let Some(popup) = view.lay.select_popup.borrow_mut().as_mut() {
                    popup.highlighted = Some(index);
                }
                self.commit_select_popup(id);
            }
            // On a group label, a disabled option or the frame: nothing.
            Some(None) => {}
            // Outside (a release with no press first): closes.
            None => {
                self.close_select_popup(id);
            }
        }
        Some(())
    }

    /// A pointer move while a drop-down is open: the popup's (the page
    /// hears nothing). Returns false when no popup is open.
    pub(crate) fn select_popup_move(&mut self, id: EngineViewId, viewport_x: f32, viewport_y: f32) -> bool {
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        let (x, y) = (viewport_x + view.scroll_offset.0, viewport_y + view.scroll_offset.1);
        let moved = match view.lay.select_popup.borrow_mut().as_mut() {
            Some(popup) => popup.hover(x, y),
            None => return false,
        };
        if moved {
            self.repaint_select_popup(id);
        }
        true
    }

    /// A wheel step while a drop-down is open: over it, its rows scroll;
    /// elsewhere it closes. `None` when no popup is open.
    pub(crate) fn select_popup_wheel(&mut self, id: EngineViewId, x: f32, y: f32, delta_y: f32) -> Option<bool> {
        let view = self.views.get(&id)?;
        let (px, py) = (x + view.scroll_offset.0, y + view.scroll_offset.1);
        let moved = {
            let mut popup = view.lay.select_popup.borrow_mut();
            let popup = popup.as_mut()?;
            if !popup.contains(px, py) {
                None
            } else {
                // Wheel sign convention: a negative delta advances.
                let rows = (-delta_y / popup.row_height).round() as isize;
                let rows = if rows == 0 && delta_y != 0.0 { -(delta_y.signum() as isize) } else { rows };
                Some(popup.scroll_rows(rows))
            }
        };
        match moved {
            Some(moved) => {
                if moved {
                    self.repaint_select_popup(id);
                }
                Some(moved)
            }
            None => Some(self.close_select_popup(id)),
        }
    }

    /// A key while a drop-down is open: the popup's (the page hears no
    /// `keydown`). `None` when no popup is open; else whether anything
    /// changed.
    pub(crate) fn select_popup_key(
        &mut self,
        id: EngineViewId,
        key_code: u32,
        key: &str,
        ctrl: bool,
        alt: bool,
    ) -> Option<bool> {
        let view = self.views.get(&id)?;
        if view.lay.select_popup.borrow().is_none() {
            return None;
        }
        let (code, typed) = key_of(key_code, key);
        enum Then {
            Repaint(bool),
            Commit,
            Close,
        }
        let then = {
            let mut popup = view.lay.select_popup.borrow_mut();
            let popup = popup.as_mut()?;
            let page = popup.visible_rows.saturating_sub(1).max(1) as isize;
            let typing = popup.typed.active(std::time::Instant::now());
            match (code, typed) {
                (VK_UP | VK_DOWN, _) if alt => Then::Commit,
                (VK_UP, _) => Then::Repaint(popup.step(-1)),
                (VK_DOWN, _) => Then::Repaint(popup.step(1)),
                (VK_PAGE_UP, _) => Then::Repaint(popup.step(-page)),
                (VK_PAGE_DOWN, _) => Then::Repaint(popup.step(page)),
                (VK_HOME, _) => Then::Repaint(popup.highlight(edge_enabled(&popup.options, false))),
                (VK_END, _) => Then::Repaint(popup.highlight(edge_enabled(&popup.options, true))),
                (VK_SPACE, _) if typing => Then::Repaint(popup.type_char(' ', std::time::Instant::now())),
                (VK_ENTER | VK_SPACE | VK_TAB | VK_F4, _) => Then::Commit,
                (VK_ESCAPE, _) => Then::Close,
                // A Ctrl (or Cmd, which arrives as Ctrl) chord is a
                // shortcut, not type-to-select.
                (0, Some(_)) if ctrl => Then::Repaint(false),
                (0, Some(c)) => Then::Repaint(popup.type_char(c, std::time::Instant::now())),
                _ => Then::Repaint(false),
            }
        };
        Some(match then {
            Then::Repaint(changed) => {
                if changed {
                    self.repaint_select_popup(id);
                }
                changed
            }
            Then::Commit => {
                self.commit_select_popup(id);
                true
            }
            Then::Close => self.close_select_popup(id),
        })
    }

    /// A key at the focused, closed drop-down `select` (after its
    /// `keydown` went uncancelled): Space, Alt+Down/Up and F4 open it;
    /// arrows, Home, End and letters pick an option directly. `None` when
    /// the key is not the select's.
    pub(crate) fn closed_select_key(
        &mut self,
        id: EngineViewId,
        select: usize,
        key_code: u32,
        key: &str,
        ctrl: bool,
        alt: bool,
    ) -> Option<bool> {
        let node = self.drop_down_select(id, select)?;
        let (code, typed) = key_of(key_code, key);
        let (options, shown) = self.select_state(id, &node);
        let target = match (code, typed) {
            (VK_UP | VK_DOWN, _) if alt => return Some(self.open_select_popup(id, select)),
            (VK_SPACE | VK_F4, _) => return Some(self.open_select_popup(id, select)),
            (VK_UP | VK_LEFT, _) => next_enabled(&options, shown, -1),
            (VK_DOWN | VK_RIGHT, _) => next_enabled(&options, shown, 1),
            (VK_HOME | VK_PAGE_UP, _) => edge_enabled(&options, false),
            (VK_END | VK_PAGE_DOWN, _) => edge_enabled(&options, true),
            // A Ctrl (or Cmd) chord is a shortcut, not type-to-select.
            (0, Some(_)) if ctrl => return None,
            (0, Some(c)) => {
                let view = self.views.get(&id)?;
                let mut typed = view.lay.select_typeahead.borrow_mut();
                if typed.0 != select {
                    *typed = (select, TypeAhead::default());
                }
                typed.1.find(&options, shown, c, std::time::Instant::now())
            }
            _ => return None,
        };
        Some(match target {
            Some(index) => self.pick_select_option(id, select, index),
            None => false,
        })
    }

    /// After a press's default action (the focus moved): a press on a
    /// drop-down select that now has the focus opens it. Its release is
    /// then the popup's to read (`select_popup_release`).
    pub(crate) fn open_select_popup_at_press(&mut self, id: EngineViewId, viewport_x: f32, viewport_y: f32) {
        let Some(focused) = self.views.get(&id).and_then(|v| v.lay.focused_node.get()).map(|n| n.raw()) else {
            return;
        };
        let Some(hit) = self
            .views
            .get(&id)
            .and_then(|v| Self::hit_at(v, viewport_x, viewport_y))
            .and_then(|h| h.node_id)
        else {
            return;
        };
        // The press was on the select itself (or on something inside it).
        let on_select = self.element_chain(id, hit).contains(&focused);
        if on_select && self.drop_down_select(id, focused).is_some() && self.open_select_popup(id, focused) {
            if let Some(view) = self.views.get(&id) {
                view.lay.select_popup_opening_press.set(true);
            }
        }
    }

    /// Close the drop-down when its select is no longer one that can show
    /// it: out of the document, no longer focused, disabled, or no longer a
    /// drop-down (`multiple`, `size` above 1). Chrome closes it then.
    pub(crate) fn close_stale_select_popup(&mut self, id: EngineViewId) {
        let Some(select) = self.views.get(&id).and_then(|v| v.lay.select_popup.borrow().as_ref().map(|p| p.select))
        else {
            return;
        };
        let focused = self.views.get(&id).and_then(|v| v.lay.focused_node.get()).map(|n| n.raw());
        let connected = self.node(id, select).is_some_and(|node| {
            let mut at = node;
            loop {
                if matches!(at.node_type, NodeType::Document) {
                    break true;
                }
                match at.parent() {
                    Some(parent) => at = parent,
                    None => break false,
                }
            }
        });
        if !connected || focused != Some(select) || self.drop_down_select(id, select).is_none() {
            debug!(?id, select, connected, ?focused, "Closing a select popup whose select changed");
            self.close_select_popup(id);
        }
    }

    /// The view takes a new size: an open drop-down closes (Chrome), unless
    /// the size is the one it has.
    pub(crate) fn close_select_popup_on_resize(&mut self, id: EngineViewId, bounds: Bounds) {
        if !self.select_popup_open(id) {
            return;
        }
        let same = self
            .stored_viewport_bounds(id)
            .is_some_and(|b| b.width == bounds.width && b.height == bounds.height);
        if !same {
            self.close_select_popup(id);
        }
    }
}
