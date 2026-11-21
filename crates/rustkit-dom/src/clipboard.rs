//! # Clipboard
//!
//! The engine's view of the system clipboard: plain text only, read and
//! written by the text controls' edit commands (`forms`), by
//! `document.execCommand` and by `navigator.clipboard`.
//!
//! [`MemoryClipboard`] is the default: an in-process store used by tests and
//! headless runs. A platform clipboard (macOS `NSPasteboard`, row CH2.E3)
//! implements [`Clipboard`] and is handed to the engine with
//! `Engine::set_clipboard`; nothing else in the engine needs to change.

use std::cell::RefCell;
use std::rc::Rc;

/// A plain-text clipboard.
///
/// Implementations are single-threaded (`Rc`), like the rest of a view's
/// state; a platform one marshals to its own thread if it must.
pub trait Clipboard {
    /// The clipboard's text, `None` when it holds no text.
    fn read_text(&self) -> Option<String>;
    /// Replace the clipboard's contents with `text`.
    fn write_text(&self, text: &str);
}

/// The clipboard a view's controls and scripts share.
pub type SharedClipboard = Rc<dyn Clipboard>;

/// An in-memory clipboard: the default for tests and headless views.
#[derive(Debug, Default)]
pub struct MemoryClipboard {
    text: RefCell<Option<String>>,
}

impl MemoryClipboard {
    /// An empty clipboard.
    pub fn new() -> Self {
        Self::default()
    }

    /// A fresh in-memory clipboard, shared.
    pub fn shared() -> SharedClipboard {
        Rc::new(Self::new())
    }
}

impl Clipboard for MemoryClipboard {
    fn read_text(&self) -> Option<String> {
        self.text.borrow().clone()
    }

    fn write_text(&self, text: &str) {
        *self.text.borrow_mut() = Some(text.to_string());
    }
}

thread_local! {
    static THREAD_DEFAULT: SharedClipboard = MemoryClipboard::shared();
}

/// This thread's default in-memory clipboard: what the clipboard-less
/// `forms::keyboard` entry points copy to and paste from.
pub fn thread_default() -> SharedClipboard {
    THREAD_DEFAULT.with(Rc::clone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_memory_clipboard_holds_the_last_text_written() {
        let cb = MemoryClipboard::new();
        assert_eq!(cb.read_text(), None);
        cb.write_text("one");
        cb.write_text("two");
        assert_eq!(cb.read_text().as_deref(), Some("two"));
    }
}
