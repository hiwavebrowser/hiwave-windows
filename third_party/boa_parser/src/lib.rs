//! Boa's **`boa_parser`** crate is a parser targeting the latest [ECMAScript language specification][spec].
//!
//! # Crate Overview
//!
//! This crate contains implementations of a [`Lexer`] and a [`Parser`] for the **ECMAScript**
//! language. The [lexical grammar][lex] and the [syntactic grammar][grammar] being targeted are
//! fully defined in the specification. See the links provided for more information.
//!
//! [spec]: https://tc39.es/ecma262
//! [lex]: https://tc39.es/ecma262/#sec-ecmascript-language-lexical-grammar
//! [grammar]: https://tc39.es/ecma262/#sec-ecmascript-language-expressions
#![doc = include_str!("../ABOUT.md")]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/boa-dev/boa/main/assets/logo_black.svg",
    html_favicon_url = "https://raw.githubusercontent.com/boa-dev/boa/main/assets/logo_black.svg"
)]
#![cfg_attr(test, allow(clippy::needless_raw_string_hashes))] // Makes strings a bit more copy-pastable
#![cfg_attr(not(test), forbid(clippy::unwrap_used))]
#![allow(
    clippy::module_name_repetitions,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    clippy::let_unit_value,
    clippy::redundant_pub_crate,
    clippy::struct_field_names
)]

pub mod error;
pub mod lexer;
pub mod parser;
pub mod source;

pub use error::Error;
pub use lexer::Lexer;
pub use parser::Parser;
pub use source::Source;

// HIWAVE PATCH (native recursion): a parse deeper than the native stack allows fails with
// `Error::is_stack_overflow` instead of overflowing the stack. An engine installs a probe that
// knows where its outermost entry began; without one a parse is measured from its own start.
mod stack_budget {
    use std::sync::atomic::{AtomicPtr, Ordering};

    /// Used when no engine probe is installed: half of a 2 MiB thread.
    const OWN_BUDGET: usize = 1024 * 1024;

    static PROBE: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

    #[inline(always)]
    pub(crate) fn stack_addr() -> usize {
        let marker = 0u8;
        std::ptr::from_ref(std::hint::black_box(&marker)) as usize
    }

    /// Install the engine's "is the native stack over its budget" probe. The probe answers `None`
    /// when its thread has no outermost engine entry (a parse outside the engine's run), and the parse
    /// is then measured from its own start, as with no probe. The probe is process-wide; the entry is
    /// per thread, so a probe that answered "not over" without an entry let such a parse recurse
    /// unmeasured (#1099 R1).
    pub fn set_stack_probe(probe: Option<fn() -> Option<bool>>) {
        let ptr = probe.map_or(std::ptr::null_mut(), |f| f as *mut ());
        PROBE.store(ptr, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn over_stack_budget(own_origin: usize) -> bool {
        let ptr = PROBE.load(Ordering::Relaxed);
        if ptr.is_null() {
            return own_origin.saturating_sub(stack_addr()) > OWN_BUDGET;
        }
        // SAFETY: only `set_stack_probe` stores here, and it stores a `fn() -> Option<bool>`.
        let probe: fn() -> Option<bool> =
            unsafe { std::mem::transmute::<*mut (), fn() -> Option<bool>>(ptr) };
        probe().unwrap_or_else(|| own_origin.saturating_sub(stack_addr()) > OWN_BUDGET)
    }
}
pub use stack_budget::set_stack_probe;
pub(crate) use stack_budget::{over_stack_budget, stack_addr};
