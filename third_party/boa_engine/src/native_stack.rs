//! HIWAVE PATCH (native recursion): a budget on the native (Rust) stack that built-ins and
//! internal methods check, so a page cannot recurse the process to death.
//!
//! Boa's JS frames live on the heap, but a built-in that recurses in Rust (JSON.stringify of
//! nested arrays, `flat`, `join` through `toString`, a Proxy or prototype chain, bound
//! functions) uses the real stack, and a 200 000-deep structure overflowed it and aborted the
//! process. Chrome throws a catchable `RangeError`. The origin is the stack address where the
//! outermost entry into the engine on this thread began; a check throws once more than the
//! budget has been used since.

use crate::{JsNativeError, JsResult};
use std::cell::Cell;

/// Default budget: the bytes of native stack one outermost engine entry may use. Half of a
/// 2 MiB thread, so debug-build frames and the host's own frames still fit under it.
pub(crate) const DEFAULT_BUDGET: usize = 1024 * 1024;

thread_local! {
    static ORIGIN: Cell<usize> = const { Cell::new(0) };
    /// The budget of the outermost entry (the parser's probe has no `Context` to ask).
    static BUDGET: Cell<usize> = const { Cell::new(DEFAULT_BUDGET) };
}

#[inline(always)]
fn here() -> usize {
    let marker = 0u8;
    std::ptr::from_ref(std::hint::black_box(&marker)) as usize
}

/// Marks the outermost entry into the engine on this thread; nested entries leave it alone.
pub(crate) struct Entry(bool);

impl Entry {
    #[inline]
    pub(crate) fn new(budget: usize) -> Self {
        ORIGIN.with(|origin| {
            if origin.get() == 0 {
                origin.set(here());
                BUDGET.with(|b| b.set(budget));
                boa_parser::set_stack_probe(Some(over_budget));
                Entry(true)
            } else {
                Entry(false)
            }
        })
    }
}

impl Drop for Entry {
    #[inline]
    fn drop(&mut self) {
        if self.0 {
            ORIGIN.with(|origin| origin.set(0));
        }
    }
}

/// Whether the native stack is past the budget of this thread's outermost entry (the parser's probe);
/// `None` when the thread has no entry, so the parser measures from its own start.
pub(crate) fn over_budget() -> Option<bool> {
    let origin = ORIGIN.with(Cell::get);
    (origin != 0).then(|| origin.saturating_sub(here()) > BUDGET.with(Cell::get))
}

/// Throws the same `RangeError` as a JS stack overflow once more than `budget` bytes of native
/// stack have been used since the outermost entry.
#[inline]
pub(crate) fn check(budget: usize) -> JsResult<()> {
    let origin = ORIGIN.with(Cell::get);
    if origin != 0 && origin.saturating_sub(here()) > budget {
        return Err(JsNativeError::range()
            .with_message("Maximum call stack size exceeded")
            .into());
    }
    Ok(())
}
