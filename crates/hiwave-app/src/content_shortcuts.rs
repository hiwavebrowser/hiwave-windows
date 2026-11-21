//! Keyboard shortcuts pressed while a page in the content view has focus.
//!
//! The ones that change what the app protects (focus mode and its blocklist, autofill) reach the
//! app through WebView2's own AcceleratorKeyPressed event on the content view's controller, which
//! the browser raises for a real key press and a page's script cannot raise. They are not served
//! as content-view IPC, which a page can send itself.

/// A shortcut the content view hands to the app natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shortcut {
    /// F11 or Ctrl+Shift+F.
    ToggleFocusMode,
    /// Ctrl+Shift+L.
    TriggerAutofill,
}

const VK_F: u32 = 0x46;
const VK_L: u32 = 0x4C;
const VK_F11: u32 = 0x7A;

/// The shortcut a key-down is, from its virtual-key code and the modifiers held.
pub(crate) fn shortcut_for(virtual_key: u32, ctrl: bool, shift: bool, alt: bool) -> Option<Shortcut> {
    match (virtual_key, ctrl, shift, alt) {
        (VK_F11, false, false, false) => Some(Shortcut::ToggleFocusMode),
        (VK_F, true, true, false) => Some(Shortcut::ToggleFocusMode),
        (VK_L, true, true, false) => Some(Shortcut::TriggerAutofill),
        _ => None,
    }
}

/// Hand the content view's shortcuts to `on` as they are pressed (Windows, WebView2).
#[cfg(target_os = "windows")]
pub(crate) fn install(webview: &wry::WebView, on: impl Fn(Shortcut) + 'static) {
    use webview2_com::AcceleratorKeyPressedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
    };
    use wry::WebViewExtWindows;

    #[link(name = "user32")]
    extern "system" {
        fn GetKeyState(virtual_key: i32) -> i16;
    }
    // The high bit of GetKeyState: the key is down.
    let held = |vk: i32| unsafe { GetKeyState(vk) } < 0;

    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
        let mut key = 0u32;
        unsafe {
            args.KeyEventKind(&mut kind)?;
            args.VirtualKey(&mut key)?;
        }
        if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN {
            return Ok(());
        }
        // VK_CONTROL, VK_SHIFT, VK_MENU (Alt).
        if let Some(shortcut) = shortcut_for(key, held(0x11), held(0x10), held(0x12)) {
            unsafe { args.SetHandled(true)? };
            on(shortcut);
        }
        Ok(())
    }));
    let mut token = Default::default();
    if let Err(e) = unsafe { webview.controller().add_AcceleratorKeyPressed(&handler, &mut token) } {
        tracing::warn!("content view shortcuts not installed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_focus_mode_and_autofill_shortcuts_are_recognised() {
        assert_eq!(shortcut_for(VK_F11, false, false, false), Some(Shortcut::ToggleFocusMode));
        assert_eq!(shortcut_for(VK_F, true, true, false), Some(Shortcut::ToggleFocusMode));
        assert_eq!(shortcut_for(VK_L, true, true, false), Some(Shortcut::TriggerAutofill));
    }

    #[test]
    fn other_keys_and_modifier_sets_are_left_to_the_page() {
        // Ctrl+F is find in page, Ctrl+L the address bar, Shift+F11 and Alt combinations nothing.
        assert_eq!(shortcut_for(VK_F, true, false, false), None);
        assert_eq!(shortcut_for(VK_L, true, false, false), None);
        assert_eq!(shortcut_for(VK_F11, false, true, false), None);
        assert_eq!(shortcut_for(VK_L, true, true, true), None);
        assert_eq!(shortcut_for(0x41, true, true, false), None);
    }
}
