//! HIWAVE PATCH: optional note when an interrupt path fires.
//!
//! The hook is a function pointer. A null hook does nothing, and the opcode
//! check calls in only after the interrupt word has already stopped the
//! turn, so a disarmed process does no extra work on the hot path.

use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

/// `site` names the path. `message` is the error text that path formats.
/// `limit` is the runtime-limit variant (`Interrupted`, `LoopIteration`, …)
/// or `-` when the path has no error value.
pub type InterruptDiag = fn(site: &'static str, message: &'static str, limit: &'static str);

static HOOK: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static LAST_WORD: AtomicU64 = AtomicU64::new(0);
static LAST_TURN: AtomicU64 = AtomicU64::new(u64::MAX);

/// Install the log hook. `None` clears it. A null hook does no work.
pub fn set_interrupt_diag(hook: Option<InterruptDiag>) {
    let ptr = match hook {
        Some(hook) => hook as *mut (),
        None => std::ptr::null_mut(),
    };
    HOOK.store(ptr, Ordering::Relaxed);
}

/// Note one cold interrupt path (await resume, a promise job, a generator,
/// a module). A null hook returns before any other work.
#[inline]
pub fn note_interrupt(site: &'static str, message: &'static str) {
    dispatch(site, message, "-");
}

/// [`note_interrupt`] plus the runtime-limit variant of `err`.
#[inline]
pub fn note_interrupt_err(site: &'static str, message: &'static str, err: &crate::JsError) {
    dispatch(site, message, limit_kind(err));
}

/// Which runtime limit `err` is, for the interrupt log.
pub fn limit_kind(err: &crate::JsError) -> &'static str {
    use crate::error::{EngineError, RuntimeLimitError};
    match err.as_engine() {
        Some(EngineError::RuntimeLimit(RuntimeLimitError::LoopIteration)) => "LoopIteration",
        Some(EngineError::RuntimeLimit(RuntimeLimitError::Interrupted)) => "Interrupted",
        Some(EngineError::RuntimeLimit(RuntimeLimitError::Recursion)) => "Recursion",
        Some(EngineError::RuntimeLimit(RuntimeLimitError::StackSize)) => "StackSize",
        Some(_) => "engine",
        None => "other",
    }
}

#[inline]
fn dispatch(site: &'static str, message: &'static str, limit: &'static str) {
    let ptr = HOOK.load(Ordering::Relaxed);
    if ptr.is_null() {
        return;
    }
    // SAFETY: `ptr` was stored from an `InterruptDiag` function pointer, or
    // it is null and we already returned. The pointer stays valid for the
    // process: the engine installs one static function.
    let hook: InterruptDiag = unsafe { std::mem::transmute(ptr) };
    hook(site, message, limit);
}

/// The opcode check stopped this turn. At most one note per `(word, turn)`,
/// so a tight loop does not log once per opcode.
#[inline]
pub fn note_interrupt_stop(word: u64, turn: u64) {
    if HOOK.load(Ordering::Relaxed).is_null() {
        return;
    }
    let prev_word = LAST_WORD.swap(word, Ordering::Relaxed);
    let prev_turn = LAST_TURN.swap(turn, Ordering::Relaxed);
    if prev_word == word && prev_turn == turn {
        return;
    }
    dispatch("vm.check_interrupt", "the script was interrupted", "Interrupted");
}
