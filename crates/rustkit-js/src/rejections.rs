//! Unhandled promise rejections: for diagnostics, and for the page's own
//! `unhandledrejection` / `rejectionhandled` events.
//!
//! Boa calls `HostPromiseRejectionTracker` when a promise is rejected with
//! no handler (`Reject`) and when a handler is later added to one
//! (`Handle`). What is still unhandled when a task's microtasks have run is
//! what a browser announces as `unhandledrejection` and reports in its
//! console.
//!
//! Two switches, both off by default; with both off the hook returns before
//! it looks at anything, so a page that rejects promises pays two flag
//! reads per rejection:
//!
//! - `tracking` (diagnostics, [`crate::JsRuntime::set_error_reports`]): the
//!   reasons are kept for [`crate::JsRuntime::take_error_reports`].
//! - `events` ([`crate::JsRuntime::set_rejection_events`]): the runtime
//!   announces each rejection to the embedder's scripts at the end of the
//!   turn it was left unhandled in (HTML "notify about rejected promises"),
//!   and a handler added after that as `rejectionhandled`. With events on, a
//!   rejection is kept for the reports only after its event, and only when
//!   no listener canceled it.
//!
//! With either on, a rejection costs a walk of the engine-owned promises
//! (the modules loaded, at most [`MAX_IGNORED`]) and of the ones pending,
//! and a handler added to a rejected promise a walk of the pending ones (a
//! turn's own, at most [`MAX_PENDING`]) and of the announced ones (at most
//! [`MAX_OUTSTANDING`]).
//!
//! The engine's own promises (a module's load and evaluation) are not the
//! page's: their rejections are reported as the module's error, so they are
//! [`forget`](RejectionHooks::forget)-ten here.
//!
//! Describing a rejection runs none of the page's code (see `diag`).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use boa_engine::builtins::promise::{OperationType, PromiseState};
use boa_engine::context::HostHooks;
use boa_engine::object::builtins::JsPromise;
use boa_engine::realm::Realm;
use boa_engine::{Context, JsObject, JsValue};

/// Most rejections held at once. With events on, everything pending is
/// announced at the end of the turn, so this bounds one turn's rejections;
/// with events off it bounds what waits for the next report. Past it the
/// oldest is dropped, and the first drop is said once in the log.
const MAX_PENDING: usize = 10_000;
/// Engine-owned promises remembered; past it the oldest is forgotten.
const MAX_IGNORED: usize = 1024;
/// Promises announced as unhandled and remembered in case a handler comes
/// (HTML's "outstanding rejected promises", which is a weak set; these are
/// strong, so there is a bound). Past it the oldest is forgotten: a handler
/// added to it later brings no `rejectionhandled`.
const MAX_OUTSTANDING: usize = 256;

/// A rejected promise, and the realm whose code rejected it: the document
/// its events are for.
pub(crate) type Rejected = (JsObject, Realm);

#[derive(Default)]
pub(crate) struct RejectionHooks {
    /// Keep reasons for the error reports.
    pub(crate) tracking: Cell<bool>,
    /// The runtime announces rejections to the embedder's scripts.
    pub(crate) events: Cell<bool>,
    pending: RefCell<VecDeque<Rejected>>,
    /// Engine-owned promises: never reported.
    ignored: RefCell<VecDeque<JsObject>>,
    /// The promise whose `unhandledrejection` is being dispatched, and
    /// whether it got a handler meanwhile.
    dispatching: RefCell<Option<(JsObject, bool)>>,
    /// Announced as unhandled; no handler since.
    outstanding: RefCell<VecDeque<Rejected>>,
    /// Announced as unhandled, then given a handler: `rejectionhandled` is owed.
    handled: RefCell<VecDeque<Rejected>>,
    /// Reasons of announced rejections, for the reports, each with whether
    /// a listener canceled its event.
    reported: RefCell<VecDeque<(JsValue, bool)>>,
    /// A pending rejection was dropped for the bound, and the log says so.
    drop_logged: Cell<bool>,
}

fn push_bounded<T>(queue: &mut VecDeque<T>, item: T, max: usize) {
    if queue.len() >= max {
        queue.pop_front();
    }
    queue.push_back(item);
}

impl HostHooks for RejectionHooks {
    fn promise_rejection_tracker(
        &self,
        promise: &JsObject<boa_engine::builtins::promise::Promise>,
        operation: OperationType,
        context: &mut Context,
    ) {
        if !self.active() {
            return;
        }
        let promise = promise.clone().upcast();
        if self.ignored.borrow().iter().any(|p| JsObject::equals(p, &promise)) {
            return;
        }
        match operation {
            OperationType::Reject => {
                let mut pending = self.pending.borrow_mut();
                if pending.len() >= MAX_PENDING && !self.drop_logged.replace(true) {
                    tracing::warn!(
                        max = MAX_PENDING,
                        "more unhandled promise rejections are pending than are kept: the oldest are dropped \
                         (no event, no report); said once"
                    );
                }
                push_bounded(&mut pending, (promise, context.realm().clone()), MAX_PENDING);
            }
            OperationType::Handle => {
                {
                    let mut pending = self.pending.borrow_mut();
                    let before = pending.len();
                    pending.retain(|(p, _)| !JsObject::equals(p, &promise));
                    if pending.len() != before || !self.events.get() {
                        return;
                    }
                }
                // Not pending: its event is being dispatched, or was.
                if let Some((dispatching, handled)) = self.dispatching.borrow_mut().as_mut() {
                    if JsObject::equals(dispatching, &promise) {
                        *handled = true;
                        return;
                    }
                }
                let mut outstanding = self.outstanding.borrow_mut();
                if let Some(at) = outstanding.iter().position(|(p, _)| JsObject::equals(p, &promise)) {
                    if let Some(entry) = outstanding.remove(at) {
                        push_bounded(&mut self.handled.borrow_mut(), entry, MAX_OUTSTANDING);
                    }
                }
            }
        }
    }
}

impl RejectionHooks {
    fn active(&self) -> bool {
        self.tracking.get() || self.events.get()
    }

    /// `promise` is the engine's own: drop it if it is pending, and never
    /// report it.
    pub(crate) fn forget(&self, promise: &JsObject) {
        if !self.active() {
            return;
        }
        self.pending.borrow_mut().retain(|(p, _)| !JsObject::equals(p, promise));
        push_bounded(&mut self.ignored.borrow_mut(), promise.clone(), MAX_IGNORED);
    }

    /// The reasons of the rejections to report, oldest first, each with
    /// whether a listener canceled its event. With events off: those still
    /// unhandled. With events on: those announced; one not announced yet
    /// stays for the end of its turn.
    pub(crate) fn take(&self) -> Vec<(JsValue, bool)> {
        let mut reasons: Vec<(JsValue, bool)> = self.reported.borrow_mut().drain(..).collect();
        if self.events.get() {
            return reasons;
        }
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        reasons.extend(pending.into_iter().filter_map(|(object, _)| reason_of(&object)).map(|reason| (reason, false)));
        reasons
    }

    /// Whether there is anything to announce.
    pub(crate) fn has_news(&self) -> bool {
        !self.pending.borrow().is_empty() || !self.handled.borrow().is_empty()
    }

    /// How many rejections are not announced yet.
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.borrow().len()
    }

    /// The oldest promise that is owed a `rejectionhandled`.
    pub(crate) fn next_handled(&self) -> Option<Rejected> {
        self.handled.borrow_mut().pop_front()
    }

    /// The oldest rejection not announced yet.
    pub(crate) fn next_pending(&self) -> Option<Rejected> {
        self.pending.borrow_mut().pop_front()
    }

    /// `promise`'s `unhandledrejection` is about to be dispatched.
    pub(crate) fn begin_dispatch(&self, promise: &JsObject) {
        *self.dispatching.borrow_mut() = Some((promise.clone(), false));
    }

    /// The dispatch is over: `reason` is kept for the reports (when they
    /// are on) with whether a listener canceled the event, and the promise is
    /// remembered for `rejectionhandled` unless a listener gave it a handler.
    pub(crate) fn end_dispatch(&self, rejected: Rejected, reason: JsValue, canceled: bool) {
        let handled = self.dispatching.borrow_mut().take().is_some_and(|(_, handled)| handled);
        if self.tracking.get() {
            push_bounded(&mut self.reported.borrow_mut(), (reason, canceled), MAX_PENDING);
        }
        if !handled {
            push_bounded(&mut self.outstanding.borrow_mut(), rejected, MAX_OUTSTANDING);
        }
    }

    /// `realm` is gone: nothing of it is announced, and nothing here keeps
    /// it alive.
    pub(crate) fn drop_realm(&self, realm: &Realm) {
        for queue in [&self.pending, &self.outstanding, &self.handled] {
            queue.borrow_mut().retain(|(_, r)| r != realm);
        }
    }
}

/// A rejected promise's reason; `None` for anything else.
pub(crate) fn reason_of(object: &JsObject) -> Option<JsValue> {
    match JsPromise::from_object(object.clone()).ok()?.state() {
        PromiseState::Rejected(reason) => Some(reason),
        _ => None,
    }
}
