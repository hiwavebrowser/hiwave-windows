//! CH2.E22: the page's focus as the cascade sees it. `:focus` matches the
//! focused element, `:focus-within` it and the elements above it (a shadow
//! root's host included), `:focus-visible` the focused element when it was
//! focused from the keyboard, or by the pointer on a control that takes
//! typed text, or by script after either (Chromium 143's heuristic,
//! `tools/parity_oracle/focus_pseudo_log.mjs`).
//!
//! The view keeps the chain and the flag (`ViewLayout::focus_chain`,
//! `focus_visible`); the build marks the elements from them like the
//! hovered chain (`live_attributes`), and a focus change is a pointer
//! restyle: the elements a focus rule can match are built again, and a
//! change of paint only skips layout (`Engine::repaint_restyled`).
//!
//! Tab and Shift+Tab move the focus through the page's focusable elements
//! (HTML §6.6.3 sequential focus navigation: a positive `tabindex` first,
//! in its order, then tree order).

use super::*;

impl Engine {
    /// The page's focus moved to `target` (raw NodeId), or away (`None`):
    /// the chain the focus pseudo-classes match, and whether the focused
    /// element matches `:focus-visible`, as the view's next build reads
    /// them. Returns whether a rule of the view's sheets can see the
    /// change (a pointer restyle is then due).
    pub(crate) fn note_page_focus(&mut self, id: EngineViewId, target: Option<usize>) -> bool {
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        let chain = target.map(|raw| Self::focus_chain_of(view, raw)).unwrap_or_default();
        let visible = target.is_some_and(|raw| !view.focus_by_pointer || Self::takes_typed_text(view, raw));
        let before = view.lay.focus_chain.borrow().clone();
        let was_visible = view.lay.focus_visible.get();
        if before == chain && was_visible == visible {
            return false;
        }
        let restyles = Self::focus_change_restyles(view, (&before, was_visible), (&chain, visible));
        let Some(view) = self.views.get_mut(&id) else {
            return false;
        };
        view.pointer_restyle |= restyles;
        *view.lay.focus_chain.borrow_mut() = chain;
        view.lay.focus_visible.set(visible);
        restyles
    }

    /// Whether some rule of the view's sheets can match differently after
    /// the focus went from `before` to `now` (chain, `:focus-visible`).
    fn focus_change_restyles(view: &ViewState, before: (&[usize], bool), now: (&[usize], bool)) -> bool {
        let index = view.lay.rule_reads.borrow();
        let Some(reads) = index.as_ref().map(|index| &index.reads) else {
            return false;
        };
        if reads.focus.anywhere || reads.focus_within.anywhere || reads.focus_visible.anywhere {
            return true;
        }
        !Self::focus_changes(view, reads, before, now).is_empty()
    }

    /// The elements a rule can restyle from a focus change, each with how
    /// many of its later siblings the rule reaches (as
    /// [`Self::pointer_changes`]): the elements that gained or lost
    /// `:focus-within`, the old and the new focused element for `:focus`,
    /// and the ones that gained or lost `:focus-visible` (whatever the
    /// sheets say: the UA's outline reads it).
    pub(crate) fn focus_changes(
        view: &ViewState,
        reads: &SelectorReads,
        before: (&[usize], bool),
        now: (&[usize], bool),
    ) -> Vec<(usize, usize)> {
        let mut changes = Vec::new();
        for raw in Self::in_one_chain(before.0, now.0) {
            changes.extend(Self::pointer_changes(view, &reads.focus_within, raw));
        }
        let (was, is) = (before.0.first().copied(), now.0.first().copied());
        if was != is {
            for raw in [was, is].into_iter().flatten() {
                changes.extend(Self::pointer_changes(view, &reads.focus, raw));
            }
        }
        // The UA's `:focus-visible` outline styles every element: one
        // that gains or loses it is always restyled.
        let (was, is) = (was.filter(|_| before.1), is.filter(|_| now.1));
        if was != is {
            for raw in [was, is].into_iter().flatten() {
                changes.push((raw, 0));
                changes.extend(Self::pointer_changes(view, &reads.focus_visible, raw));
            }
        }
        changes
    }

    /// An element and the elements above it, innermost first, crossing
    /// from a shadow root to its host.
    fn focus_chain_of(view: &ViewState, raw: usize) -> Vec<usize> {
        let mut chain = Vec::new();
        let mut node = view.document.as_ref().and_then(|d| d.get_node(rustkit_dom::NodeId::new(raw)));
        while let Some(n) = node {
            if n.is_element() {
                chain.push(n.id.raw());
            }
            node = n.parent().or_else(|| n.shadow_host());
        }
        chain
    }

    /// Whether the element takes typed text (Chromium's
    /// `MayTriggerVirtualKeyboard`): a text-like `<input>`, a `<textarea>`,
    /// an editing host. A pointer focus on one matches `:focus-visible`.
    fn takes_typed_text(view: &ViewState, raw: usize) -> bool {
        let Some(node) = view.document.as_ref().and_then(|d| d.get_node(rustkit_dom::NodeId::new(raw))) else {
            return false;
        };
        let NodeType::Element { tag_name, attributes, .. } = &node.node_type else {
            return false;
        };
        match tag_name.to_lowercase().as_str() {
            "textarea" => true,
            "input" => {
                let kind = attributes.get("type").map(|t| t.to_ascii_lowercase()).unwrap_or_default();
                !matches!(
                    kind.as_str(),
                    "checkbox" | "radio" | "button" | "submit" | "reset" | "image" | "file" | "color" | "range" | "hidden"
                )
            }
            _ => Self::editing_host_of(&node).is_some_and(|(host, _)| host == raw),
        }
    }

    /// The element's tab index when it is focusable (HTML §6.6.2, the
    /// cases the bindings' `isFocusable` takes): its `tabindex`, else 0
    /// for a control that is not disabled, a link, an editing host.
    pub(crate) fn focus_tab_index(node: &Rc<Node>) -> Option<i32> {
        let NodeType::Element { tag_name, attributes, .. } = &node.node_type else {
            return None;
        };
        let tag = tag_name.to_lowercase();
        let control = matches!(tag.as_str(), "input" | "select" | "textarea" | "button");
        if control && attributes.contains_key("disabled") {
            return None;
        }
        if let Some(index) = attributes.get("tabindex").and_then(|t| t.trim().parse::<i32>().ok()) {
            return Some(index);
        }
        let focusable = match tag.as_str() {
            "input" => !attributes.get("type").is_some_and(|t| t.eq_ignore_ascii_case("hidden")),
            "select" | "textarea" | "button" => true,
            "a" | "area" => attributes.contains_key("href"),
            _ => Self::editing_host_of(node).is_some_and(|(host, _)| host == node.id.raw()),
        };
        focusable.then_some(0)
    }

    /// The element a press on `node` focuses: it or the nearest element
    /// above it that is focusable (a click in a button's label focuses
    /// the button). `None`: the press takes the focus away.
    pub(crate) fn pointer_focus_target(node: &Rc<Node>) -> Option<usize> {
        let mut at = Some(node.clone());
        while let Some(n) = at {
            if Self::focus_tab_index(&n).is_some() && !Self::focus_blocked(&n) {
                return Some(n.id.raw());
            }
            at = n.parent();
        }
        None
    }

    /// Whether an element is kept from the focus by what is above it: an
    /// `inert` ancestor (or itself), or, for a form control, a disabled
    /// `<fieldset>` it is in outside that fieldset's first `<legend>`
    /// (HTML §4.10.18.2; Chromium 143 skips both in the Tab order).
    fn focus_blocked(node: &Rc<Node>) -> bool {
        let control = matches!(
            &node.node_type,
            NodeType::Element { tag_name, .. }
                if matches!(tag_name.to_lowercase().as_str(), "input" | "select" | "textarea" | "button")
        );
        let mut child = node.clone();
        let mut at = Some(node.clone());
        while let Some(n) = at {
            if let NodeType::Element { tag_name, attributes, .. } = &n.node_type {
                if attributes.contains_key("inert") {
                    return true;
                }
                if control && !Rc::ptr_eq(&n, node) && tag_name.eq_ignore_ascii_case("fieldset") && attributes.contains_key("disabled") {
                    let first_legend = n.children().into_iter().find(|c| {
                        matches!(&c.node_type, NodeType::Element { tag_name, .. } if tag_name.eq_ignore_ascii_case("legend"))
                    });
                    if !first_legend.is_some_and(|legend| Rc::ptr_eq(&legend, &child)) {
                        return true;
                    }
                }
            }
            child = n.clone();
            at = n.parent().or_else(|| n.shadow_host());
        }
        false
    }

    /// The page's focused element (raw NodeId): a button or a link too,
    /// which take no keys as a control does.
    pub(crate) fn page_focus(&self, id: EngineViewId) -> Option<usize> {
        self.views.get(&id)?.lay.focus_chain.borrow().first().copied()
    }

    /// A button Enter and Space click (Chromium 143): `<button>` and the
    /// button-like `<input>` types, not disabled.
    pub(crate) fn is_key_activated_button(&self, id: EngineViewId, raw: usize) -> bool {
        let Some(node) = self.node(id, raw) else {
            return false;
        };
        let NodeType::Element { tag_name, attributes, .. } = &node.node_type else {
            return false;
        };
        if attributes.contains_key("disabled") {
            return false;
        }
        match tag_name.to_lowercase().as_str() {
            "button" => true,
            "input" => attributes
                .get("type")
                .is_some_and(|t| matches!(t.to_ascii_lowercase().as_str(), "button" | "submit" | "reset")),
            _ => false,
        }
    }

    /// Tab (Shift+Tab: `backward`): the focus moves to the next (previous)
    /// element of the page's sequential focus order, from the focused one
    /// (from the start, or the end, when nothing is focused). Past the
    /// last one the page keeps no focus. The focus is the keyboard's, so
    /// it matches `:focus-visible`.
    pub(crate) fn sequential_focus(&mut self, id: EngineViewId, backward: bool) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let Some(document) = view.document.clone() else {
            return;
        };
        let layout = view.layout.clone();
        // A focusable element that is rendered (it has a box: not
        // `display: none`, which a `[hidden]` element is unless the author
        // shows it) and visible, not inert or in a disabled fieldset, with
        // tabindex >= 0, in tree order. Shadow trees are not walked: the
        // page's focus (the bindings) cannot hold an element in one yet
        // (no retargeting, `ShadowRoot.activeElement` is null), so Tab
        // skips them.
        let mut order: Vec<(usize, i32)> = Vec::new();
        fn walk(node: &Rc<Node>, layout: Option<&LayoutBox>, order: &mut Vec<(usize, i32)>) {
            if let Some(index) = Engine::focus_tab_index(node).filter(|i| *i >= 0) {
                let raw = node.id.raw();
                let shown = match layout {
                    None => true,
                    Some(layout) => box_of_node(layout, raw)
                        .is_some_and(|b| b.style.visibility == rustkit_css::Visibility::Visible),
                };
                if shown && !Engine::focus_blocked(node) {
                    order.push((raw, index));
                }
            }
            for child in node.children() {
                walk(&child, layout, order);
            }
        }
        walk(&document.root(), layout.as_deref(), &mut order);
        // A positive tabindex comes first, in its order; then the rest.
        order.sort_by_key(|(_, index)| if *index > 0 { (0, *index) } else { (1, 0) });
        let current = view.lay.focus_chain.borrow().first().copied();
        let at = current.and_then(|raw| order.iter().position(|(r, _)| *r == raw));
        let next = match (at, backward) {
            (Some(at), false) => order.get(at + 1),
            (Some(at), true) => at.checked_sub(1).and_then(|at| order.get(at)),
            (None, false) => order.first(),
            (None, true) => order.last(),
        }
        .map(|(raw, _)| *raw);
        if let Some(view) = self.views.get_mut(&id) {
            view.focus_by_pointer = false;
        }
        self.move_focus(id, next);
    }

    /// Focus `target` (raw NodeId) as the user does: a control or an
    /// editing host takes the keys, the page's focus follows (its
    /// listeners run), and the view is restyled when a rule reads it.
    fn move_focus(&mut self, id: EngineViewId, target: Option<usize>) {
        let control = target.and_then(|raw| {
            let node = self.node(id, raw)?;
            Self::form_control(&node).or_else(|| Self::editing_host_of(&node).filter(|(host, _)| *host == raw))
        });
        let before = self.views.get(&id).and_then(|v| v.lay.focused_node.get());
        self.set_focus(id, control);
        // The control that took (or lost) the keys paints its caret.
        if let Some(view) = self.views.get_mut(&id) {
            view.pointer_restyle |= before != view.lay.focused_node.get();
        }
        let told = self.views.get(&id).and_then(|v| v.bindings.as_ref()).map(|bindings| {
            if let Err(e) = bindings.set_focus(target) {
                debug!(?id, error = %e, "focus listener threw");
            }
        });
        if told.is_some() {
            if let Err(e) = self.flush_script_dom_writes(id, Some(RelayoutCause::HoverOrFocus)) {
                debug!(?id, error = %e, "relayout after focus events failed");
            }
        } else {
            self.note_page_focus(id, target);
        }
        self.settle_pointer_restyle(id);
    }
}
