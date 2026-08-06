//! Tab bookkeeping for the native shell — pure model, no Win32.
//!
//! The native shell's tab handling splits in two deliberately:
//!
//! - **This file** owns the bookkeeping: which views exist, which one is
//!   active, what happens to the selection when a tab closes. No windows, no
//!   GPU, no engine — so it is unit-testable on any machine, including CI
//!   runners with no display. Every ordering rule below is pinned by a test.
//! - **`win32.rs`** owns the side effects: creating/destroying engine views,
//!   showing and hiding them, keeping `ViewType::Content` pointed at whatever
//!   this model says is active.
//!
//! The split exists because the interesting bugs in tab handling are ordering
//! bugs (close the middle tab, which one is selected?), and ordering bugs do
//! not need a GPU to reproduce — but they do go untested if the only way to
//! run the code is to open a window.

use std::fmt::Debug;

/// What a tab points at. Generic rather than hard-wired to `EngineViewId`
/// because the strip's rules are pure index arithmetic — and because a
/// production API should not grow a `from_raw` constructor merely so its
/// tests can name a value. The shell instantiates this as
/// `TabModel<EngineViewId>`; the tests use plain integers.
pub trait TabId: Copy + PartialEq + Debug {}
impl<T: Copy + PartialEq + Debug> TabId for T {}

/// Which tab becomes active after the active one closes.
///
/// Chrome-shaped: the RIGHT neighbour inherits selection, falling back to the
/// left when the closed tab was last. Codified here rather than left to index
/// arithmetic because "which tab did I just land on" is the whole user-visible
/// behaviour of closing a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabSwitch<Id: TabId> {
    /// The view that was showing before this operation.
    pub previous: Id,
    /// The view that should be showing now.
    pub current: Id,
}

/// Ordered tab strip with exactly one active tab, always.
#[derive(Debug)]
pub struct TabModel<Id: TabId> {
    views: Vec<Id>,
    active: usize,
}

impl<Id: TabId> TabModel<Id> {
    /// A window opens with one tab. There is no such thing as a zero-tab
    /// window in this model, which is why `new` takes a view rather than
    /// defaulting to empty — the invariant is established by construction
    /// instead of being asserted afterwards.
    pub fn new(first: Id) -> Self {
        Self {
            views: vec![first],
            active: 0,
        }
    }

    pub fn count(&self) -> usize {
        self.views.len()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    /// The currently showing view. Total: the model cannot be empty.
    pub fn active_view(&self) -> Id {
        self.views[self.active]
    }

    pub fn views(&self) -> &[Id] {
        &self.views
    }

    /// Append a tab and select it. Returns the switch the shell must apply.
    pub fn push(&mut self, view: Id) -> TabSwitch<Id> {
        let previous = self.active_view();
        self.views.push(view);
        self.active = self.views.len() - 1;
        TabSwitch {
            previous,
            current: view,
        }
    }

    /// Close the active tab.
    ///
    /// Returns `None` when this is the last tab — **the model never empties
    /// itself.** Closing the final tab is a window-level decision (quit? open
    /// a blank tab?) and belongs to the shell, not to the tab strip. Returning
    /// `None` instead of silently leaving zero tabs is what stops
    /// `active_view()` from being a panic waiting for a user to find it.
    pub fn close_active(&mut self) -> Option<(Id, TabSwitch<Id>)> {
        if self.views.len() <= 1 {
            return None;
        }
        let closed = self.views.remove(self.active);
        // The right neighbour has now slid into `self.active`; clamp handles
        // the case where the closed tab was last.
        if self.active >= self.views.len() {
            self.active = self.views.len() - 1;
        }
        Some((
            closed,
            TabSwitch {
                previous: closed,
                current: self.active_view(),
            },
        ))
    }

    /// Select a tab by index.
    ///
    /// `None` for an out-of-range index or a no-op re-selection of the active
    /// tab: both mean the shell has nothing to show or hide. Out-of-range is
    /// deliberately not an error — index-keyed activation arrives from
    /// keyboard shortcuts (Ctrl+1..9), and Ctrl+7 with three tabs open is a
    /// gesture at nothing, not a fault.
    pub fn activate(&mut self, index: usize) -> Option<TabSwitch<Id>> {
        if index >= self.views.len() || index == self.active {
            return None;
        }
        let previous = self.active_view();
        self.active = index;
        Some(TabSwitch {
            previous,
            current: self.active_view(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests name views as plain integers — see the `TabId` note above.
    fn v(n: u64) -> u64 {
        n
    }

    fn model_of(ids: &[u64]) -> TabModel<u64> {
        let mut m = TabModel::new(v(ids[0]));
        for id in &ids[1..] {
            m.push(v(*id));
        }
        m
    }

    #[test]
    fn a_window_opens_with_exactly_one_active_tab() {
        let m = TabModel::new(v(1));
        assert_eq!(m.count(), 1);
        assert_eq!(m.active_view(), v(1));
        assert_eq!(m.active_index(), 0);
    }

    #[test]
    fn a_new_tab_is_appended_and_selected() {
        let mut m = TabModel::new(v(1));
        let switch = m.push(v(2));
        assert_eq!(switch.previous, v(1));
        assert_eq!(switch.current, v(2));
        assert_eq!(m.active_view(), v(2));
        assert_eq!(m.views(), &[v(1), v(2)]);
    }

    #[test]
    fn closing_the_active_middle_tab_selects_the_right_neighbour() {
        let mut m = model_of(&[1, 2, 3]);
        m.activate(1).expect("select the middle tab");
        let (closed, switch) = m.close_active().expect("not the last tab");
        assert_eq!(closed, v(2));
        assert_eq!(switch.current, v(3), "right neighbour inherits selection");
        assert_eq!(m.views(), &[v(1), v(3)]);
        assert_eq!(m.active_index(), 1);
    }

    #[test]
    fn closing_the_active_last_tab_falls_back_to_the_left() {
        let mut m = model_of(&[1, 2, 3]);
        // push() left tab 3 active, which is also the last one — the clamp
        // path. Without it, `active` would index one past the end.
        let (closed, switch) = m.close_active().expect("not the last tab");
        assert_eq!(closed, v(3));
        assert_eq!(switch.current, v(2));
        assert_eq!(m.active_index(), 1);
    }

    #[test]
    fn the_model_never_empties_itself() {
        let mut m = TabModel::new(v(1));
        assert!(
            m.close_active().is_none(),
            "closing the final tab is the shell's decision, not the strip's"
        );
        // The load-bearing consequence: active_view() is still total.
        assert_eq!(m.active_view(), v(1));
        assert_eq!(m.count(), 1);
    }

    #[test]
    fn activating_out_of_range_changes_nothing() {
        let mut m = model_of(&[1, 2]);
        m.activate(0).expect("select the first tab");
        assert!(m.activate(9).is_none(), "Ctrl+9 with two tabs open");
        assert_eq!(m.active_view(), v(1), "selection is untouched");
        assert_eq!(m.count(), 2);
    }

    #[test]
    fn reselecting_the_active_tab_is_not_a_switch() {
        let mut m = model_of(&[1, 2]);
        assert!(
            m.activate(1).is_none(),
            "no switch means the shell hides nothing — hiding then showing \
             the same view is a visible flicker"
        );
        assert_eq!(m.active_view(), v(2));
    }

    #[test]
    fn closing_down_to_one_tab_still_leaves_that_tab_active() {
        let mut m = model_of(&[1, 2, 3]);
        m.close_active().expect("3 -> 2");
        m.close_active().expect("2 -> 1");
        assert_eq!(m.count(), 1);
        assert!(m.close_active().is_none());
        assert_eq!(m.active_view(), v(1));
    }
}
