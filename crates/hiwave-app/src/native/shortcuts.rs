//! Keyboard shortcuts for the native shell — pure resolution, no Win32.
//!
//! Same split as `tabs.rs`, for the same reason: which chord means what is a
//! table, and a table can be tested without opening a window. `win32.rs` owns
//! only the plumbing (subscribe, drain, dispatch).
//!
//! WHY THIS UNIT EXISTS: before it, the four capabilities this browser had
//! just brought home from Chromium — back, forward, reload, and tabs — were
//! reachable **only** by an IPC message from chrome HTML. A person sitting at
//! the keyboard could not use any of them. The viewhost had been emitting
//! fully-formed key events with modifiers the whole time and nothing was
//! listening: the orphan-wiring pattern this fleet keeps finding, once more.

use rustkit_core::input::{KeyCode, Modifiers};

/// A shell action bound to a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    NewTab,
    CloseTab,
    /// Zero-based tab index (Ctrl+1 is index 0).
    ActivateTab(usize),
    /// Ctrl+9 is "last tab", not "tab nine" — matching every other browser.
    ActivateLastTab,
    Back,
    Forward,
    Reload,
    Stop,
}

/// Resolve a key press to a shell action.
///
/// Returns `None` for anything unbound, which the caller must treat as "let
/// the page have it" — a browser that swallows unrecognised keys breaks every
/// text field on the web.
pub fn resolve(key: KeyCode, mods: Modifiers) -> Option<Shortcut> {
    // The Windows key is never part of a browser chord: those belong to the
    // shell (Win+D, Win+L) and stealing them is hostile.
    if mods.meta {
        return None;
    }

    if mods.ctrl && !mods.alt {
        // Ctrl+Shift+<something> is a DIFFERENT chord space (reopen closed
        // tab, private window). Unbound here rather than falling through to
        // the unshifted meaning — Ctrl+Shift+T must never silently open a
        // plain new tab.
        if mods.shift {
            return None;
        }
        return match key {
            KeyCode::KeyT => Some(Shortcut::NewTab),
            KeyCode::KeyW => Some(Shortcut::CloseTab),
            KeyCode::KeyR => Some(Shortcut::Reload),
            KeyCode::Digit1 => Some(Shortcut::ActivateTab(0)),
            KeyCode::Digit2 => Some(Shortcut::ActivateTab(1)),
            KeyCode::Digit3 => Some(Shortcut::ActivateTab(2)),
            KeyCode::Digit4 => Some(Shortcut::ActivateTab(3)),
            KeyCode::Digit5 => Some(Shortcut::ActivateTab(4)),
            KeyCode::Digit6 => Some(Shortcut::ActivateTab(5)),
            KeyCode::Digit7 => Some(Shortcut::ActivateTab(6)),
            KeyCode::Digit8 => Some(Shortcut::ActivateTab(7)),
            KeyCode::Digit9 => Some(Shortcut::ActivateLastTab),
            _ => None,
        };
    }

    if mods.alt && !mods.ctrl && !mods.shift {
        return match key {
            KeyCode::ArrowLeft => Some(Shortcut::Back),
            KeyCode::ArrowRight => Some(Shortcut::Forward),
            _ => None,
        };
    }

    if !mods.ctrl && !mods.alt && !mods.shift {
        return match key {
            KeyCode::F5 => Some(Shortcut::Reload),
            KeyCode::Escape => Some(Shortcut::Stop),
            _ => None,
        };
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    fn alt() -> Modifiers {
        Modifiers {
            alt: true,
            ..Default::default()
        }
    }

    fn none() -> Modifiers {
        Modifiers::default()
    }

    #[test]
    fn the_tab_chords_resolve() {
        assert_eq!(resolve(KeyCode::KeyT, ctrl()), Some(Shortcut::NewTab));
        assert_eq!(resolve(KeyCode::KeyW, ctrl()), Some(Shortcut::CloseTab));
    }

    #[test]
    fn the_navigation_chords_resolve() {
        assert_eq!(resolve(KeyCode::ArrowLeft, alt()), Some(Shortcut::Back));
        assert_eq!(resolve(KeyCode::ArrowRight, alt()), Some(Shortcut::Forward));
        assert_eq!(resolve(KeyCode::KeyR, ctrl()), Some(Shortcut::Reload));
        assert_eq!(resolve(KeyCode::F5, none()), Some(Shortcut::Reload));
        assert_eq!(resolve(KeyCode::Escape, none()), Some(Shortcut::Stop));
    }

    #[test]
    fn ctrl_digits_are_zero_based_tab_indices() {
        assert_eq!(
            resolve(KeyCode::Digit1, ctrl()),
            Some(Shortcut::ActivateTab(0)),
            "Ctrl+1 is the FIRST tab, index 0"
        );
        assert_eq!(
            resolve(KeyCode::Digit8, ctrl()),
            Some(Shortcut::ActivateTab(7))
        );
    }

    #[test]
    fn ctrl_9_is_the_last_tab_not_the_ninth() {
        assert_eq!(
            resolve(KeyCode::Digit9, ctrl()),
            Some(Shortcut::ActivateLastTab),
            "every other browser does this; a literal index 8 would be wrong"
        );
    }

    #[test]
    fn an_unbound_key_is_left_for_the_page() {
        assert_eq!(resolve(KeyCode::KeyA, none()), None);
        assert_eq!(
            resolve(KeyCode::KeyA, ctrl()),
            None,
            "Ctrl+A is select-all and belongs to the page, not the shell"
        );
    }

    #[test]
    fn ctrl_shift_is_a_different_chord_space() {
        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            resolve(KeyCode::KeyT, ctrl_shift),
            None,
            "Ctrl+Shift+T is reopen-closed-tab; it must NEVER fall through to \
             plain new-tab"
        );
        assert_eq!(resolve(KeyCode::Digit1, ctrl_shift), None);
    }

    #[test]
    fn the_windows_key_is_never_ours() {
        let meta_ctrl = Modifiers {
            ctrl: true,
            meta: true,
            ..Default::default()
        };
        assert_eq!(
            resolve(KeyCode::KeyT, meta_ctrl),
            None,
            "Win+chords belong to the desktop shell"
        );
    }

    #[test]
    fn alt_arrows_need_alt_alone() {
        let ctrl_alt = Modifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        assert_eq!(
            resolve(KeyCode::ArrowLeft, ctrl_alt),
            None,
            "Ctrl+Alt+Left is a desktop rotate chord on some drivers"
        );
    }
}
