//! Script source kept only while an embedder asks for it, so a later error
//! report can quote the text around a position. Off by default: nothing is
//! stored, and [`ScriptSources::get`] is `None`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

/// How many scripts to keep, and how much of their text. A newer script
/// drops the oldest. One script larger than the byte cap is not stored.
const MAX_SOURCES: usize = 24;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct ScriptSources {
    on: Cell<bool>,
    bytes: Cell<usize>,
    entries: RefCell<VecDeque<(String, String)>>,
}

impl ScriptSources {
    pub(crate) fn set_enabled(&self, on: bool) {
        self.on.set(on);
        if !on {
            self.entries.borrow_mut().clear();
            self.bytes.set(0);
        }
    }

    pub(crate) fn remember(&self, path: &str, source: &str) {
        if !self.on.get() || path.is_empty() || source.is_empty() || source.len() > MAX_BYTES {
            return;
        }
        let mut entries = self.entries.borrow_mut();
        if let Some(index) = entries.iter().position(|(name, _)| name == path) {
            let (_, old) = entries.remove(index).expect("index was just found");
            self.bytes.set(self.bytes.get().saturating_sub(old.len()));
        }
        while !entries.is_empty()
            && (entries.len() >= MAX_SOURCES || self.bytes.get().saturating_add(source.len()) > MAX_BYTES)
        {
            let (_, old) = entries.pop_front().expect("not empty");
            self.bytes.set(self.bytes.get().saturating_sub(old.len()));
        }
        self.bytes.set(self.bytes.get().saturating_add(source.len()));
        entries.push_back((path.to_string(), source.to_string()));
    }

    pub(crate) fn get(&self, path: &str) -> Option<String> {
        if !self.on.get() {
            return None;
        }
        self.entries.borrow().iter().rev().find(|(name, _)| name == path).map(|(_, source)| source.clone())
    }
}
