//! Unhandled promise rejections, for diagnostics.
//!
//! Boa calls `HostPromiseRejectionTracker` when a promise is rejected with
//! no handler (`Reject`) and when a handler is later added to one
//! (`Handle`). What is still unhandled when the embedder asks
//! ([`crate::JsRuntime::take_error_reports`], after an evaluation and
//! its jobs) is what a browser reports as `unhandledrejection`.
//!
//! Off by default: the hook then returns before it looks at anything, so a
//! page that rejects promises pays one flag read per rejection.
//!
//! The engine's own promises (a module's load and evaluation) are not the
//! page's: their rejections are reported as the module's error, so they are
//! [`forget`](RejectionHooks::forget)-ten here.
//!
//! Describing a rejection runs none of the page's code (see `diag`).

use std::cell::{Cell, RefCell};

use boa_engine::builtins::promise::{OperationType, PromiseState};
use boa_engine::context::HostHooks;
use boa_engine::object::builtins::JsPromise;
use boa_engine::{Context, JsObject, JsValue};

/// Most rejections held at once; past it the oldest is dropped (reported
/// nowhere: a page rejecting this many promises per evaluation is already
/// named by the ones that were kept).
const MAX_PENDING: usize = 256;
/// Engine-owned promises remembered; past it the oldest is forgotten.
const MAX_IGNORED: usize = 1024;

#[derive(Default)]
pub(crate) struct RejectionHooks {
    pub(crate) tracking: Cell<bool>,
    pending: RefCell<std::collections::VecDeque<JsObject>>,
    /// Engine-owned promises: never reported.
    ignored: RefCell<std::collections::VecDeque<JsObject>>,
}

impl HostHooks for RejectionHooks {
    fn promise_rejection_tracker(
        &self,
        promise: &JsObject<boa_engine::builtins::promise::Promise>,
        operation: OperationType,
        _context: &mut Context,
    ) {
        if !self.tracking.get() {
            return;
        }
        let promise = promise.clone().upcast();
        if self.ignored.borrow().iter().any(|p| JsObject::equals(p, &promise)) {
            return;
        }
        let mut pending = self.pending.borrow_mut();
        match operation {
            OperationType::Reject => {
                if pending.len() >= MAX_PENDING {
                    pending.pop_front();
                }
                pending.push_back(promise);
            }
            OperationType::Handle => pending.retain(|p| !JsObject::equals(p, &promise)),
        }
    }
}

impl RejectionHooks {
    /// `promise` is the engine's own: drop it if it is pending, and never
    /// report it.
    pub(crate) fn forget(&self, promise: &JsObject) {
        if !self.tracking.get() {
            return;
        }
        self.pending.borrow_mut().retain(|p| !JsObject::equals(p, promise));
        let mut ignored = self.ignored.borrow_mut();
        if ignored.len() >= MAX_IGNORED {
            ignored.pop_front();
        }
        ignored.push_back(promise.clone());
    }

    /// The reasons of the rejections still unhandled, oldest first.
    pub(crate) fn take(&self) -> Vec<JsValue> {
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        pending
            .into_iter()
            .filter_map(|object| {
                let promise = JsPromise::from_object(object).ok()?;
                match promise.state() {
                    PromiseState::Rejected(reason) => Some(reason),
                    _ => None,
                }
            })
            .collect()
    }
}
