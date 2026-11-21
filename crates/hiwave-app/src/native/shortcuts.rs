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
    /// Open the find bar (the chrome page owns it).
    Find,
    /// Focus the address bar.
    FocusAddressBar,
    ToggleSidebar,
    /// Open the command palette (the shelf).
    CommandPalette,
    /// Open the history panel.
    History,
    /// Toggle focus mode (Ctrl+Shift+F, F11).
    FocusMode,
}

/// Whether a shortcut types into the chrome page (the find bar, the address bar, the
/// sidebar, the history panel). Focus mode hides all of that (its own CSS hides the find
/// overlay and the toolbar), so these chords would send keystrokes into UI nobody can see
/// (Pollux's R1 note on #128): they leave focus mode first.
pub fn needs_chrome_visible(shortcut: Shortcut) -> bool {
    matches!(
        shortcut,
        Shortcut::Find | Shortcut::CommandPalette | Shortcut::FocusAddressBar | Shortcut::History | Shortcut::ToggleSidebar
    )
}

/// Escape in focus mode leaves it, unless a page is loading (then it stops the load, as it
/// does everywhere else).
pub fn escape_leaves_focus_mode(in_focus_mode: bool, loading: bool) -> bool {
    in_focus_mode && !loading
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
            // The one Ctrl+Shift chord the shell owns: focus mode.
            return (key == KeyCode::KeyF && !mods.alt).then_some(Shortcut::FocusMode);
        }
        return match key {
            KeyCode::KeyT => Some(Shortcut::NewTab),
            KeyCode::KeyW => Some(Shortcut::CloseTab),
            KeyCode::KeyR => Some(Shortcut::Reload),
            KeyCode::KeyF => Some(Shortcut::Find),
            KeyCode::KeyL => Some(Shortcut::FocusAddressBar),
            KeyCode::KeyB => Some(Shortcut::ToggleSidebar),
            KeyCode::KeyK => Some(Shortcut::CommandPalette),
            KeyCode::KeyH => Some(Shortcut::History),
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
            KeyCode::F11 => Some(Shortcut::FocusMode),
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
    fn the_chrome_chords_resolve() {
        assert_eq!(resolve(KeyCode::KeyF, ctrl()), Some(Shortcut::Find));
        assert_eq!(resolve(KeyCode::KeyL, ctrl()), Some(Shortcut::FocusAddressBar));
        assert_eq!(resolve(KeyCode::KeyB, ctrl()), Some(Shortcut::ToggleSidebar));
        assert_eq!(resolve(KeyCode::KeyK, ctrl()), Some(Shortcut::CommandPalette));
        assert_eq!(resolve(KeyCode::KeyH, ctrl()), Some(Shortcut::History));
    }

    #[test]
    fn chords_that_type_into_the_chrome_need_it_visible() {
        for s in [Shortcut::Find, Shortcut::CommandPalette, Shortcut::FocusAddressBar, Shortcut::History, Shortcut::ToggleSidebar] {
            assert!(needs_chrome_visible(s), "{s:?}");
        }
        for s in [Shortcut::Reload, Shortcut::NewTab, Shortcut::CloseTab, Shortcut::Stop, Shortcut::FocusMode] {
            assert!(!needs_chrome_visible(s), "{s:?}");
        }
    }

    #[test]
    fn escape_leaves_focus_mode_unless_a_page_is_loading() {
        assert!(escape_leaves_focus_mode(true, false));
        assert!(!escape_leaves_focus_mode(true, true), "Escape stops the load first");
        assert!(!escape_leaves_focus_mode(false, false), "outside focus mode it is just Stop");
    }

    #[test]
    fn ctrl_shift_f_and_f11_are_focus_mode_a_bare_f_is_typing() {
        let ctrl_shift = Modifiers { ctrl: true, shift: true, ..Default::default() };
        assert_eq!(resolve(KeyCode::KeyF, ctrl_shift), Some(Shortcut::FocusMode));
        assert_eq!(resolve(KeyCode::F11, none()), Some(Shortcut::FocusMode));
        assert_eq!(resolve(KeyCode::KeyF, none()), None, "a bare F is typing");
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
