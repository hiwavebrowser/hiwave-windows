//! The engine half of the script-network bridge.
//!
//! Bindings queue what a page script asks for (`web_net_bridge.js`); this
//! module takes the queue, runs every request under the page's
//! [`FetchPolicy`] (the one allow/deny layer, in rustkit-net) through the
//! engine's `ResourceLoader`, and delivers each outcome back. It adds no
//! rule of its own: a request the policy refuses is an error to the page,
//! and the page sees only "network error", never the reason (which would
//! tell a script what is on the user's network).
//!
//! Determinism: requests are taken in the order the page made them,
//! fetched concurrently (the policy bounds the concurrency), and delivered
//! in that same order, so a run does not depend on which response was
//! fastest.

use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Poll;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use rustkit_bindings::{DomBindings, NetDelivery, NetRequest};
use rustkit_net::policy::{FetchPolicy, RequestMode, ResponseKind, ScriptRequest, ScriptResponse};
use rustkit_net::{CredentialsMode, RedirectMode, Request, RequestDestination, RequestId, ResourceLoader};
use tracing::debug;
use url::Url;

use crate::inspector::{self, FailureStage, Inspector, ResourceType};

/// What the page sees for every refusal and every failure.
const NETWORK_ERROR: &str = "network error";

fn log_request(method: &str, url: &str, status: &str, disposition: &str) {
    crate::console_log::net(
        "end",
        method,
        url,
        status,
        disposition,
        rustkit_js::diag_nav(),
    );
}

fn log_handoff(request: &NetRequest, outcome: &NetDelivery, disposition: &str) {
    if !crate::console_log::enabled() {
        return;
    }
    let (status, url) = match outcome {
        NetDelivery::Response { status, url, .. } => (status.to_string(), url.as_str()),
        NetDelivery::Error(_) => ("-".to_string(), request.url.as_str()),
    };
    log_request(&request.method, url, &status, disposition);
}

/// A live-loop answer no longer has the request's method. The start line does.
pub(crate) fn log_live_handoff(outcome: &NetDelivery, disposition: &str) {
    if !crate::console_log::enabled() {
        return;
    }
    match outcome {
        NetDelivery::Response { url, status, .. } => {
            log_request("-", url, &status.to_string(), disposition);
        }
        NetDelivery::Error(_) => log_request("-", "-", "-", disposition),
    }
}

fn log_denial(method: &str, url: &str, denial: &rustkit_net::policy::Denial) {
    if !crate::console_log::enabled() {
        return;
    }
    use rustkit_net::policy::Denial;
    let (disposition, status) = match denial {
        Denial::Shield(_) | Denial::Csp => ("blocked", "-".to_string()),
        Denial::Timeout => ("timeout", "-".to_string()),
        Denial::Cancelled => ("cancelled", "-".to_string()),
        Denial::BadStatus(code) => ("response", code.to_string()),
        Denial::Network(_) => return,
        _ => ("error", "-".to_string()),
    };
    log_request(method, url, &status, disposition);
}

thread_local! {
    /// Answers fetched while the JavaScript budget was already spent. The
    /// load hands them to the next live turn; failing them here would reject
    /// the page's promise, and delivering them would resume it under a
    /// deadline that has already passed.
    static PARKED: std::cell::RefCell<Vec<(u64, NetDelivery)>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) fn clear_parked_deliveries() {
    PARKED.with(|parked| parked.borrow_mut().clear());
}

pub(crate) fn take_parked_deliveries() -> Vec<(u64, NetDelivery)> {
    PARKED.with(|parked| std::mem::take(&mut *parked.borrow_mut()))
}

fn park_delivery(delivery: (u64, NetDelivery)) {
    PARKED.with(|parked| parked.borrow_mut().push(delivery));
}

/// What one pump found.
#[derive(Debug, Default)]
pub(crate) struct Pump {
    /// The JS engine panicked: its state is unknowable, so the caller runs
    /// nothing more on this page.
    pub poisoned: bool,
    /// Errors the bridge's own JS calls raised (a callback's exception is
    /// caught inside the bridge and lands in the page's error queue).
    pub threw: Vec<String>,
    /// Requests taken and fetched.
    pub requests: usize,
    /// Rounds that did work.
    pub rounds: u32,
    /// What each inserted script did, for the script log.
    pub records: Vec<crate::ScriptRecord>,
}

/// Run take / fetch / deliver rounds until the queue is empty or `rounds`
/// rounds have run. `timers` is the virtual-clock horizon and callback cap
/// to run after each round's deliveries (a response handler may schedule
/// work); `None` delivers only.
///
/// Whatever is still waiting when the rounds or the deadline run out
/// completes with a network error. `deadline` is a network cap: it does
/// not arm the watchdog and it does not refuse a host call. A delivery
/// whose JavaScript budget is already spent is parked for a later turn
/// instead of being run or failed.
pub(crate) async fn pump(
    bindings: &DomBindings,
    policy: &FetchPolicy,
    loader: &ResourceLoader,
    deadline: tokio::time::Instant,
    rounds: u32,
    timers: Option<(u64, u32)>,
    inspector: &Arc<Inspector>,
) -> Pump {
    let mut out = Pump::default();

    for _ in 0..rounds {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let Ok(requests) = catch_unwind(AssertUnwindSafe(|| bindings.take_net_requests())) else {
            out.poisoned = true;
            return out;
        };
        if requests.is_empty() {
            return out;
        }
        out.rounds += 1;
        out.requests += requests.len();

        let outcomes = futures::future::join_all(requests.iter().map(|request| {
            execute(
                policy,
                loader,
                deadline,
                request,
                inspector,
                FailureStage::Timeout,
            )
        }))
        .await;

        // In the order the page made them (the take order), not completion
        // order. JavaScript budget spent: keep the callback and the answer
        // for a turn that still has some. Delivering now would resume the
        // page under a stop, and the bridge would forget the request.
        for (request, outcome) in requests.iter().zip(outcomes) {
            // Exhausted, or so little left that resuming the page's async
            // function would land in a turn the watchdog stops at once.
            if super::script_js_exhausted() || super::script_js_resume_would_be_stopped() {
                if bindings.shelve_net_request(request.id) {
                    log_handoff(request, &outcome, "parked");
                    park_delivery((request.id, outcome));
                }
                continue;
            }
            log_handoff(request, &outcome, "delivered");
            let (delivered, stopped) = super::with_script_js(bindings, || {
                catch_unwind(AssertUnwindSafe(|| {
                    bindings.deliver_net_response(request.id, outcome)
                }))
            });
            match delivered {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    if !stopped {
                        out.threw.push(e.to_string());
                    }
                }
                Err(_) => {
                    out.poisoned = true;
                    return out;
                }
            }
        }
        if let Some((horizon_ms, max_callbacks)) = timers {
            if !super::script_js_exhausted() {
                let (ran, stopped) = super::with_script_js(bindings, || {
                    catch_unwind(AssertUnwindSafe(|| {
                        bindings.run_timers(horizon_ms, max_callbacks)
                    }))
                });
                match ran {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        if !stopped {
                            out.threw.push(e.to_string());
                        }
                    }
                    Err(_) => {
                        out.poisoned = true;
                        return out;
                    }
                }
            }
        }
    }

    // Out of rounds or time. Nothing may be left waiting forever.
    if catch_unwind(AssertUnwindSafe(|| bindings.fail_pending_net_requests(NETWORK_ERROR))).is_err() {
        out.poisoned = true;
    }
    out
}

/// Work the live loop has started and not yet taken the result of. It runs
/// only while [`settle_live`] polls it; dropping it abandons it.
pub(crate) type LiveFuture<T> = Pin<Box<dyn Future<Output = T>>>;

/// A script request that is out: its id and, in the end, its outcome.
pub(crate) type LiveRequest = LiveFuture<(u64, NetDelivery)>;

/// Whether live script requests run on the network runtime (T7). On
/// unless `HIWAVE_NET_RUNTIME=0`, which selects the page-polled path of
/// before, for bisects and the A/B.
fn net_runtime_wanted() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| net_runtime_switch(std::env::var("HIWAVE_NET_RUNTIME").ok().as_deref()))
}

/// `HIWAVE_NET_RUNTIME`'s value read as the switch: off only for `0`,
/// `off` or `false`; unset, empty and anything else are on.
pub(crate) fn net_runtime_switch(value: Option<&str>) -> bool {
    !matches!(value, Some("0" | "off" | "false"))
}

/// How many `rustkit-net` workers: `HIWAVE_NET_WORKERS`, 1 to 8, default 2.
/// `1` is T7a's one current-thread runtime (D1), kept for the A/B.
fn net_workers() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| net_workers_switch(std::env::var("HIWAVE_NET_WORKERS").ok().as_deref()))
}

/// `HIWAVE_NET_WORKERS`'s value read as a worker count: a number from 1 to
/// 8; unset, empty or anything else is 2.
pub(crate) fn net_workers_switch(value: Option<&str>) -> usize {
    value.and_then(|v| v.trim().parse().ok()).filter(|n| (1..=8).contains(n)).unwrap_or(2)
}

/// The process's network runtime (T7): `rustkit-net` workers (two by
/// default, [`net_workers`]) that run live script requests, so their bytes
/// are read and their response and failure records written while the page
/// thread is busy. Built on first use. `None` when it is switched off or
/// could not be built; the page then polls its requests itself, as before.
/// It is never shut down: requests still out at process exit end
/// unrecorded, with the ring that would have held their records.
///
/// The worker count does not reorder requests (T8): the policy's per-origin
/// slots are FIFO semaphores, and [`start_live`] takes each request's place
/// in them on the page thread ([`FetchPolicy::reserve`]), in the order the
/// page started them. Before that the place was taken at the task's first
/// poll, which two workers raced (T2's combined fixture: `/hang/4` capped
/// instead of `/hang/6`), so T7a ran one thread (D1). [`DiagContext`] is
/// set per poll, which a work-stealing runtime needs. What runs here:
/// socket waits, TLS handshakes, body decode, the base64 of [`delivery`],
/// Shield matching and the Inspector records. Nothing runs a page script.
fn net_runtime() -> Option<&'static tokio::runtime::Handle> {
    static NET: OnceLock<Option<tokio::runtime::Handle>> = OnceLock::new();
    #[cfg(test)]
    if PAGE_POLLED.with(std::cell::Cell::get) {
        return None;
    }
    if !net_runtime_wanted() {
        return None;
    }
    NET.get_or_init(|| {
        let workers = net_workers();
        let mut builder = if workers == 1 {
            tokio::runtime::Builder::new_current_thread()
        } else {
            let mut b = tokio::runtime::Builder::new_multi_thread();
            b.worker_threads(workers).thread_name("rustkit-net");
            b
        };
        let runtime = builder
            .enable_all()
            .build()
            .map_err(|e| tracing::warn!(error = %e, "network runtime not built; the page polls its requests"))
            .ok()?;
        let handle = runtime.handle().clone();
        std::thread::Builder::new()
            .name("rustkit-net".into())
            .spawn(move || runtime.block_on(std::future::pending::<()>()))
            .map_err(|e| tracing::warn!(error = %e, "network thread not started; the page polls its requests"))
            .ok()?;
        Some(handle)
    })
    .as_ref()
}

/// A task on the network runtime, aborted when its page side is dropped (a
/// navigation cleared the live requests, or the view closed). Abort is
/// cooperative: a task that already finished has written its record.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Runs `inner` with the page's diagnostic nav id (and, in tests, the
/// `HIWAVE_CONSOLE_LOG` switch and sink) set around every poll, restoring
/// the thread's own after, also when the poll panics: the one `rustkit-net`
/// thread runs every view's tasks, so between two polls of this task it
/// polls other pages' (T7 B2). Taken on the page thread when the task is
/// spawned. A task the runtime spawns on its own (the h2 connection driver)
/// runs outside it and writes no mirror line.
struct DiagContext<F> {
    nav: u64,
    #[cfg(test)]
    hooks: crate::console_log::TestHooks,
    inner: Pin<Box<F>>,
}

impl<F: Future> DiagContext<F> {
    fn new(inner: F) -> Self {
        DiagContext {
            nav: rustkit_js::diag_nav(),
            #[cfg(test)]
            hooks: crate::console_log::test_hooks(),
            inner: Box::pin(inner),
        }
    }
}

impl<F: Future> Future for DiagContext<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<F::Output> {
        let _restore = Restore {
            nav: rustkit_js::diag_nav(),
            #[cfg(test)]
            hooks: Some(crate::console_log::swap_test_hooks(self.hooks.clone())),
        };
        rustkit_js::set_diag_nav(self.nav);
        self.inner.as_mut().poll(cx)
    }
}

/// Puts the thread's own diagnostic context back when a [`DiagContext`]
/// poll ends, by return or by unwind.
struct Restore {
    nav: u64,
    #[cfg(test)]
    hooks: Option<crate::console_log::TestHooks>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        #[cfg(test)]
        if let Some(hooks) = self.hooks.take() {
            crate::console_log::swap_test_hooks(hooks);
        }
        rustkit_js::set_diag_nav(self.nav);
    }
}

/// What the engine records when a request task panics: engine words, never
/// the page's.
const TASK_PANICKED: &str = "engine failure: request task panicked";

#[cfg(test)]
thread_local! {
    /// `HIWAVE_NET_RUNTIME=0` for requests this thread starts (T7 test 9).
    static PAGE_POLLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn force_page_polled_for_test(on: bool) {
    PAGE_POLLED.with(|cell| cell.set(on));
}

/// A request to this path panics where the policy runs (T7 test 10, C3).
#[cfg(test)]
pub(crate) const TASK_PANIC_PATH: &str = "/__t7_task_panic";

/// A request to this path fails, on the network runtime, with a reason that
/// holds its full URL, query and all (T7 test 7: the scrub at record time).
#[cfg(test)]
pub(crate) const FAIL_NAMING_URL_PATH: &str = "/__t7_fail_naming_url";

/// Most live script requests one view has out on the network runtime
/// (T7 S2). In flight is capped by the policy (24 per page, 6 per origin),
/// but a task parked behind those caps still holds its request; this bounds
/// what one page can park in the process-wide runtime. Ten times the page
/// cap, so a page within the policy's caps never meets it.
pub(crate) const MAX_LIVE_NET_TASKS: usize = 240;

/// The reason a request past [`MAX_LIVE_NET_TASKS`] is refused with, as the
/// inserted-script queue says it.
const TOO_MANY_REQUESTS: &str = "too many script requests";

/// Start `request` for the live loop; it fails as a network error at
/// `deadline`. On the network runtime the request record is made here, on
/// the page thread at request start (its document, the URL scrub), and the
/// rest runs and is recorded on a `rustkit-net` worker; [`settle_live`]
/// only takes the outcome. Without it, nothing runs until [`settle_live`]
/// polls it. Which of the two is chosen before the request record, so it is
/// never made twice (T7 C2). `out` is how many the view has out already:
/// past [`MAX_LIVE_NET_TASKS`] nothing is spawned and the request fails at
/// once, recorded on the page.
pub(crate) fn start_live(
    policy: Arc<FetchPolicy>,
    loader: Arc<ResourceLoader>,
    request: NetRequest,
    deadline: tokio::time::Instant,
    inspector: Arc<Inspector>,
    out: usize,
) -> LiveRequest {
    let id = request.id;
    let Some(net) = net_runtime() else {
        return Box::pin(async move {
            let outcome = execute(&policy, &loader, deadline, &request, &inspector, FailureStage::Timeout).await;
            (id, outcome)
        });
    };
    let started = match begin(&request, &inspector, deadline) {
        Ok(started) => started,
        Err(refused) => return Box::pin(std::future::ready((id, refused))),
    };
    if out >= MAX_LIVE_NET_TASKS {
        debug!(id, out, "Script request over the view's task cap");
        log_request(&started.method, &started.url, "-", "error");
        if let Some(guard) = started.guard {
            guard.fail(FailureStage::Policy, TOO_MANY_REQUESTS);
        }
        return Box::pin(std::future::ready((id, NetDelivery::Error(NETWORK_ERROR.into()))));
    }
    // The slot ticket (T8): the budget, the shape check and the place in the
    // origin's slot queue are taken here, on the page thread, in the order
    // the page started its requests, so the net runtime's worker count does
    // not reorder them. A refusal is recorded here, as the task would have.
    let mut started = started;
    let wait = started.guard.as_ref().and_then(|g| g.slot()).cloned();
    match policy.reserve(&started.script_request, wait.as_ref()) {
        Ok(reservation) => started.reservation = Some(reservation),
        Err(denial) => {
            let Started { id, guard, method, url, .. } = started;
            return Box::pin(std::future::ready((id, denied(id, &method, &url, guard, &denial))));
        }
    }
    let (deliver, outcome) = tokio::sync::oneshot::channel();
    let task = net.spawn(DiagContext::new(async move {
        let outcome = finish(&policy, &loader, deadline, started, FailureStage::Timeout).await;
        // The page side reads the channel, so the outcome is there before
        // the poke: the page's next live turn takes it now, not at its next
        // scheduled turn (T7 S1), and never finds it still pending.
        if deliver.send(outcome).is_ok() {
            poke_live_turn();
        }
    }));
    let task = AbortOnDrop(task);
    Box::pin(async move {
        let _task = task;
        match outcome.await {
            Ok(outcome) => (id, outcome),
            // The task ended without sending: a panic is answered in the
            // task, so this is the belt. An unexpected cancel (the runtime
            // is never shut down) is the same network error (T7 C2).
            Err(_) => {
                tracing::warn!(id, "live request task ended without an outcome");
                (id, NetDelivery::Error(NETWORK_ERROR.into()))
            }
        }
    })
}

/// Let what is out make progress for at most `slice`, and take the results
/// that are ready, in the order the work was started. Returns as soon as one
/// is; the rest stays in `in_flight` for a later turn.
///
/// The work lives on the runtime that polls it: every turn of one view must
/// come from the same runtime.
pub(crate) async fn settle_live<T>(in_flight: &mut Vec<LiveFuture<T>>, slice: std::time::Duration) -> Vec<T> {
    let mut done = Vec::new();
    if in_flight.is_empty() {
        return done;
    }
    let pause = tokio::time::sleep(slice);
    tokio::pin!(pause);
    std::future::poll_fn(|cx| {
        let mut i = 0;
        while i < in_flight.len() {
            match in_flight[i].as_mut().poll(cx) {
                Poll::Ready(outcome) => {
                    drop(in_flight.remove(i));
                    done.push(outcome);
                }
                Poll::Pending => i += 1,
            }
        }
        if !done.is_empty() {
            return Poll::Ready(());
        }
        pause.as_mut().poll(cx)
    })
    .await;
    done
}

/// One script request through the policy. The Inspector sees it from here
/// (a request refused before the policy, as malformed, was never made);
/// `on_deadline` is the stage recorded when `deadline` passes first. Dropped
/// before it ends (the page navigated away), it is recorded cancelled.
async fn execute(
    policy: &FetchPolicy,
    loader: &ResourceLoader,
    deadline: tokio::time::Instant,
    request: &NetRequest,
    inspector: &Arc<Inspector>,
    on_deadline: FailureStage,
) -> NetDelivery {
    match begin(request, inspector, deadline) {
        Ok(started) => finish(policy, loader, deadline, started, on_deadline).await,
        Err(refused) => refused,
    }
}

/// A script request whose request record is made, not yet sent.
struct Started {
    id: u64,
    script_request: ScriptRequest,
    guard: Option<inspector::RequestGuard>,
    /// For the `HIWAVE_CONSOLE_LOG` lines; empty when that is off.
    method: String,
    url: String,
    /// Its place in the policy's slot queues, when taken where it started
    /// (T8); otherwise [`finish`] takes it when the request first runs.
    reservation: Option<rustkit_net::policy::Reservation>,
}

/// The start of [`execute`], on the page thread: refuse what is malformed,
/// and make the request record (its document, the URL scrub).
fn begin(request: &NetRequest, inspector: &Arc<Inspector>, deadline: tokio::time::Instant) -> Result<Started, NetDelivery> {
    let script_request = match script_request(request) {
        Ok(r) => r,
        Err(reason) => {
            debug!(id = request.id, %reason, "Script request refused before the policy");
            log_request(&request.method, &request.url, "-", "error");
            return Err(NetDelivery::Error(NETWORK_ERROR.into()));
        }
    };
    let log_net = crate::console_log::enabled();
    let method = if log_net { script_request.method.to_string() } else { String::new() };
    let url = if log_net { script_request.url.to_string() } else { String::new() };
    let (resource_type, initiator) = match script_request.destination {
        RequestDestination::Xhr => (ResourceType::Xhr, "xhr"),
        RequestDestination::Fetch => (ResourceType::Fetch, "fetch"),
        _ => (ResourceType::Other, "script"),
    };
    let guard = inspector
        .begin_request(&script_request.url, script_request.method.as_str(), resource_type, Some(initiator))
        .map(|g| {
            g.dropped_as(FailureStage::Cancelled, "abandoned (navigation or view closed)")
                .waits_on(&rustkit_net::policy::SlotWait::new())
                .deadline(deadline)
        });
    Ok(Started { id: request.id, script_request, guard, method, url, reservation: None })
}

/// The rest of [`execute`]: send, and record the response or failure where
/// it runs. A panic in the policy is a failure in engine words, never
/// "abandoned": the guard is held outside the unwind (T7 B3a). A panic in
/// `guard.response`/`guard.fail` themselves is outside it (T7 C3).
async fn finish(
    policy: &FetchPolicy,
    loader: &ResourceLoader,
    deadline: tokio::time::Instant,
    started: Started,
    on_deadline: FailureStage,
) -> NetDelivery {
    use futures::FutureExt as _;
    let Started { id, script_request, guard, method, url, reservation } = started;
    let wait = guard.as_ref().and_then(|g| g.slot()).cloned();
    #[cfg(test)]
    if script_request.url.path() == FAIL_NAMING_URL_PATH {
        if let Some(guard) = guard {
            guard.fail(FailureStage::Policy, &format!("refused {}", script_request.url));
        }
        return NetDelivery::Error(NETWORK_ERROR.into());
    }
    let sent = AssertUnwindSafe(async {
        #[cfg(test)]
        if script_request.url.path() == TASK_PANIC_PATH {
            panic!("injected request task panic");
        }
        match reservation {
            Some(reservation) => {
                let sent = policy.execute_reserved(loader, script_request, reservation, wait.as_ref());
                tokio::time::timeout_at(deadline, sent).await
            }
            None => tokio::time::timeout_at(deadline, policy.execute_waited(loader, script_request, wait.as_ref())).await,
        }
    })
    .catch_unwind()
    .await;
    let Ok(sent) = sent else {
        tracing::warn!(id, "{TASK_PANICKED}");
        if let Some(guard) = guard {
            guard.fail(FailureStage::Policy, TASK_PANICKED);
        }
        return NetDelivery::Error(NETWORK_ERROR.into());
    };
    match sent {
        Ok(Ok(response)) => {
            if let Some(guard) = guard {
                let mime = response.headers.get("content-type").and_then(|v| v.to_str().ok());
                guard.response(response.status, mime, None, Some(response.body.len() as u64), false);
            }
            delivery(response)
        }
        Ok(Err(denial)) => denied(id, &method, &url, guard, &denial),
        Err(_) => {
            debug!(id, "Script request hit its network cap");
            log_request(&method, &url, "-", "timeout");
            if let Some(guard) = guard {
                let reason = match on_deadline {
                    FailureStage::OverBudget => "script budget spent",
                    _ => "timed out",
                };
                guard.fail(on_deadline, reason);
            }
            NetDelivery::Error(NETWORK_ERROR.into())
        }
    }
}

/// A request the policy refused, logged and recorded where it was refused.
fn denied(
    id: u64,
    method: &str,
    url: &str,
    guard: Option<inspector::RequestGuard>,
    denial: &rustkit_net::policy::Denial,
) -> NetDelivery {
    debug!(id, ?denial, "Script request denied");
    log_denial(method, url, denial);
    if let Some(guard) = guard {
        let (stage, reason) = inspector::denial_stage(denial);
        guard.blocked_by(inspector::denial_block_reason(denial)).fail(stage, reason);
    }
    NetDelivery::Error(NETWORK_ERROR.into())
}

/// The bridge's plain data as the policy's request type. Anything malformed
/// is refused here, before the policy; the policy still vets everything else.
fn script_request(request: &NetRequest) -> Result<ScriptRequest, String> {
    let method = Method::from_bytes(request.method.as_bytes()).map_err(|_| "bad method".to_string())?;
    let url = Url::parse(&request.url).map_err(|_| "unparseable url".to_string())?;

    let mut headers = HeaderMap::new();
    for (name, value) in &request.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| "bad header name".to_string())?;
        let value = HeaderValue::from_str(value).map_err(|_| "bad header value".to_string())?;
        headers.append(name, value);
    }

    let body = match &request.body_b64 {
        Some(b64) => Some(Bytes::from(B64.decode(b64).map_err(|_| "bad body encoding".to_string())?)),
        None => None,
    };

    let mode = match request.mode.as_str() {
        "cors" => RequestMode::Cors,
        "no-cors" => RequestMode::NoCors,
        "same-origin" => RequestMode::SameOrigin,
        _ => return Err("bad mode".into()),
    };
    let credentials = match request.credentials.as_str() {
        "omit" => CredentialsMode::Omit,
        "same-origin" => CredentialsMode::SameOrigin,
        "include" => CredentialsMode::Include,
        _ => return Err("bad credentials".into()),
    };
    let redirect = match request.redirect.as_str() {
        "follow" => RedirectMode::Follow,
        "manual" => RedirectMode::Manual,
        "error" => RedirectMode::Error,
        _ => return Err("bad redirect".into()),
    };
    let destination = match request.destination.as_str() {
        "xhr" => RequestDestination::Xhr,
        "fetch" => RequestDestination::Fetch,
        _ => return Err("bad destination".into()),
    };

    Ok(ScriptRequest {
        id: RequestId::new(),
        method,
        url,
        headers,
        body,
        mode,
        credentials,
        redirect,
        destination,
    })
}

fn delivery(response: ScriptResponse) -> NetDelivery {
    NetDelivery::Response {
        url: response.url.to_string(),
        status: response.status,
        status_text: response.status_text,
        headers: response
            .headers
            .iter()
            .filter_map(|(name, value)| Some((name.as_str().to_string(), value.to_str().ok()?.to_string())))
            .collect(),
        body_b64: B64.encode(&response.body),
        kind: match response.kind {
            ResponseKind::Basic => "basic",
            ResponseKind::Cors => "cors",
            ResponseKind::Opaque => "opaque",
            ResponseKind::OpaqueRedirect => "opaqueredirect",
        }
        .to_string(),
        redirected: response.redirected,
    }
}

/// Most modules one page may pull in. The policy has its own request caps;
/// this bounds the graph walk itself (a page cannot keep the engine here with
/// a generated module graph).
pub(crate) const MAX_MODULES_PER_PAGE: usize = 2_000;

/// Deepest import chain followed (each round fetches one level, all its
/// modules concurrently).
const MAX_MODULE_ROUNDS: u32 = 64;

/// Fetch what a started module graph has asked for, level by level, until it
/// asks for nothing more. Every fetch is the policy's module entry point;
/// this decides nothing. A module that is refused, mistyped, missing or too
/// many is supplied as an error, so the graph fails as a whole and none of
/// its code runs.
///
/// `fetched` counts the modules already pulled in on this page.
pub(crate) async fn pump_modules(
    bindings: &DomBindings,
    policy: &FetchPolicy,
    loader: &ResourceLoader,
    deadline: tokio::time::Instant,
    document: &Url,
    fetched: &mut usize,
    inspector: &Arc<Inspector>,
) -> Pump {
    let mut out = Pump::default();
    for _ in 0..MAX_MODULE_ROUNDS {
        let Ok(urls) = catch_unwind(AssertUnwindSafe(|| bindings.take_module_requests())) else {
            out.poisoned = true;
            return out;
        };
        if urls.is_empty() {
            return out;
        }
        out.rounds += 1;
        out.requests += urls.len();

        // Within the page's module cap, fetch the level concurrently.
        let room = MAX_MODULES_PER_PAGE.saturating_sub(*fetched);
        *fetched += urls.len().min(room);
        let outcomes = futures::future::join_all(urls.iter().enumerate().map(|(i, url)| async move {
            if i >= room {
                return Err(format!("too many modules (limit {MAX_MODULES_PER_PAGE})"));
            }
            fetch_module(policy, loader, url, document, deadline, inspector).await
        }))
        .await;

        for (url, outcome) in urls.iter().zip(outcomes) {
            let (supplied, _) = super::with_script_js(bindings, || {
                catch_unwind(AssertUnwindSafe(|| bindings.supply_module(url, outcome)))
            });
            if supplied.is_err() {
                out.poisoned = true;
                return out;
            }
        }
    }
    out
}

/// One module through the policy's module entry point, as the page gets
/// it: its source, or why there is none.
async fn fetch_module(
    policy: &FetchPolicy,
    loader: &ResourceLoader,
    url: &str,
    document: &Url,
    deadline: tokio::time::Instant,
    inspector: &Arc<Inspector>,
) -> Result<rustkit_bindings::FetchedModule, String> {
    let parsed = Url::parse(url).map_err(|_| "unparseable module url".to_string())?;
    let guard = inspector
        .begin_request(&parsed, "GET", ResourceType::Module, Some("script"))
        .map(|g| g.waits_on(&rustkit_net::policy::SlotWait::new()).deadline(deadline));
    let wait = guard.as_ref().and_then(|g| g.slot()).cloned();
    let fetched =
        tokio::time::timeout_at(deadline, policy.fetch_module_waited(loader, &parsed, document, wait.as_ref())).await;
    inspector::answer_module(guard, &fetched);
    match fetched {
        Ok(Ok(module)) => Ok(rustkit_bindings::FetchedModule {
            final_url: module.final_url.to_string(),
            source: module.source,
        }),
        Ok(Err(denial)) => {
            debug!(%url, ?denial, "Module fetch denied");
            Err(NETWORK_ERROR.to_string())
        }
        Err(_) => Err("timed out".to_string()),
    }
}

/// A module a dynamic `import()` asked for that is out: the URL as asked
/// for and, in the end, its source or why there is none.
pub(crate) type LiveModule = LiveFuture<(String, Result<rustkit_bindings::FetchedModule, String>)>;

/// Start the modules `urls` for the live loop, as [`start_live`] starts a
/// script request: nothing runs until [`settle_live`] polls them, and no
/// turn waits for one. Past the page's module cap (`fetched` counts what
/// this document has started), a module fails at once.
pub(crate) fn start_live_modules(
    policy: &Arc<FetchPolicy>,
    loader: &Arc<ResourceLoader>,
    urls: Vec<String>,
    document: &Url,
    deadline: tokio::time::Instant,
    fetched: &mut usize,
    inspector: &Arc<Inspector>,
) -> Vec<LiveModule> {
    urls.into_iter()
        .map(|url| -> LiveModule {
            if *fetched >= MAX_MODULES_PER_PAGE {
                let refused = Err(format!("too many modules (limit {MAX_MODULES_PER_PAGE})"));
                return Box::pin(std::future::ready((url, refused)));
            }
            *fetched += 1;
            let (policy, loader, document, inspector) =
                (policy.clone(), loader.clone(), document.clone(), inspector.clone());
            Box::pin(async move {
                let outcome = fetch_module(&policy, &loader, &url, &document, deadline, &inspector).await;
                (url, outcome)
            })
        })
        .collect()
}

/// Take-fetch-deliver for the network a page can have waiting: script
/// requests (XHR, fetch) and the modules a dynamic `import()` asked for.
/// One can start the other, so they alternate until none found work, at
/// most `ALTERNATIONS` times. Whatever is still waiting then is left for
/// the next step's pump (the network side fails its stragglers itself).
///
/// Dynamically inserted external scripts are not part of this.
/// [`drive_inserted_scripts`] fetches them. They do not delay
/// `DOMContentLoaded`. Ones prepared while the document was still loading
/// do delay `window` `load`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn pump_all(
    bindings: &DomBindings,
    policy: &FetchPolicy,
    loader: &ResourceLoader,
    deadline: tokio::time::Instant,
    rounds: u32,
    timers: Option<(u64, u32)>,
    document: Option<&Url>,
    modules_fetched: &mut usize,
    inspector: &Arc<Inspector>,
    requests: bool,
) -> Pump {
    const ALTERNATIONS: u32 = 4;
    let mut total = Pump::default();
    for _ in 0..ALTERNATIONS {
        // Without `requests` the page's fetches stay queued for the live
        // loop (`pump_live` takes them), and nothing here fails them.
        let net = match requests {
            true => pump(bindings, policy, loader, deadline, rounds, timers, inspector).await,
            false => Pump::default(),
        };
        let modules = match document {
            Some(document) => {
                pump_modules(bindings, policy, loader, deadline, document, modules_fetched, inspector).await
            }
            None => Pump::default(),
        };
        let worked = net.requests + modules.requests;
        total.poisoned |= net.poisoned || modules.poisoned;
        total.threw.extend(net.threw);
        total.threw.extend(modules.threw);
        total.requests += worked;
        total.rounds += net.rounds + modules.rounds;
        if total.poisoned || worked == 0 {
            break;
        }
    }
    total
}

/// Deepest chain of inserted scripts followed in one drive (each round
/// fetches what the previous round's scripts inserted).
const MAX_INSERTED_SCRIPT_ROUNDS: u32 = 16;

/// The window loop's wake. Set by the app; tests leave it empty and call
/// `pump_live` themselves. The callback only wakes the loop. It does not
/// run the script.
static LIVE_WAKE: OnceLock<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>> = OnceLock::new();

/// Install `wake`. A later installed callback replaces the earlier one.
pub(crate) fn set_live_wake(wake: Arc<dyn Fn() + Send + Sync>) {
    let slot = LIVE_WAKE.get_or_init(|| Mutex::new(None));
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake);
}

pub(crate) fn poke_live_turn() {
    let Some(slot) = LIVE_WAKE.get() else {
        return;
    };
    let wake = slot.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(wake) = wake {
        wake();
    }
}

/// External classic scripts one view has inserted and not yet run. The
/// fetch runs on the engine's fetch thread, not on the turn that enqueued
/// it. A result is stored here and a live turn runs it. A script runs when
/// its own fetch finishes, unless `async` was set false, in which case it
/// waits for the in-order scripts inserted before it. A fetch that failed
/// or was blocked fires `error` as soon as that result is in and does not
/// hold later scripts. `retire` (a new document) drops whatever is queued
/// and rejects a result stamped with an earlier `nav_id`, so it cannot run
/// on the next page.
pub(crate) struct InsertedScripts {
    board: Arc<Mutex<InsertedBoard>>,
}

struct InsertedBoard {
    /// The document these jobs belong to. A result stamped with another
    /// id is discarded.
    nav_id: u64,
    next_id: u64,
    /// How many results have been stored. A ready script records the value
    /// at the moment its body arrived, so two that are both waiting run in
    /// the order they finished.
    ready_seq: u64,
    jobs: Vec<InsertedJob>,
    /// Events a script has earned and could not hear yet because the turn's
    /// JavaScript budget was spent, oldest first. A turn with budget fires
    /// them before it runs another script, so each element hears exactly one
    /// `load` or `error` (#775: a loader waiting on a dropped one waited
    /// forever).
    owed: std::collections::VecDeque<OwedEvent>,
    notify: Arc<tokio::sync::Notify>,
}

struct OwedEvent {
    node: usize,
    event: &'static str,
    /// The script's URL, for the log.
    source: String,
    /// The index of this element's own record in the pump that ran it,
    /// when the event fires in that pump. `None` once it is owed to a
    /// later turn, whose pump does not hold that record.
    record: Option<usize>,
}

struct InsertedJob {
    id: u64,
    node: usize,
    url: String,
    nav_id: u64,
    /// `async` was set false: this script waits for in-order scripts
    /// inserted before it. Otherwise it runs when its fetch finishes.
    in_order: bool,
    /// Set when `body` is stored. Zero while the fetch is still out.
    ready_seq: u64,
    body: Option<FetchedInserted>,
    guard: Option<crate::inspector::RequestGuard>,
    /// Tripped on navigation or a cancelled load. The in-flight fetch
    /// selects on it and drops the socket.
    cancel: Arc<FetchCancel>,
    /// From the page inserting it to the engine taking it from the queue.
    queued: std::time::Duration,
    /// When the engine took it (its request started then), and when its
    /// body or failure was stored.
    requested_at: std::time::Instant,
    arrived_at: Option<std::time::Instant>,
    /// Records its `cancelled` end when the job is dropped unrun.
    inspector: Arc<Inspector>,
}

/// How many inserted-script fetches may be on the wire at once. Script
/// fetches that crowd the wire starve a page's stylesheets (#733's
/// microsoft gate); Chrome holds an HTTP/1.1 host to six connections.
pub(crate) const MAX_INSERTED_FETCHES_IN_FLIGHT: usize = 8;
/// How many may be waiting behind those. Past this the script hears
/// `error` instead of the queue growing without a bound. The queue is the
/// process's, not a view's: 64 failed a burst of about 73 (walmart inserts
/// about 113 at once). Per-view bounds come with M6's tabs.
const MAX_INSERTED_FETCH_QUEUE: usize = 512;

struct FetchCancel {
    flag: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl FetchCancel {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            flag: std::sync::atomic::AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        })
    }

    fn cancel(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    fn is_cancelled(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        loop {
            // Register before the check, so a cancel that lands between
            // them still wakes this wait.
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

fn cancelled_fetch() -> FetchedInserted {
    FetchedInserted::Transport {
        stage: FailureStage::Cancelled,
        inspector_reason: "abandoned (navigation or view closed)",
        detail: "cancelled".into(),
        blocked_by: None,
    }
}

fn cancel_inserted_jobs(board: &mut InsertedBoard) {
    for job in board.jobs.drain(..) {
        job.cancel.cancel();
        if let Some(guard) = job.guard {
            guard.fail(FailureStage::Cancelled, "abandoned (navigation or view closed)");
        }
        job.inspector.script_cancelled(&job.url, "inserted", Some(job.queued));
    }
    board.owed.clear();
    board.notify.notify_waiters();
}

/// What the fetch thread produced. The script itself runs later, inside a
/// turn.
enum FetchedInserted {
    Source(InsertedSource),
    /// A response that was not 2xx. The element hears `error`.
    Http(InsertedHttp),
    Transport {
        stage: FailureStage,
        inspector_reason: &'static str,
        detail: String,
        /// The shield's list and rule, when it refused the fetch and said.
        blocked_by: Option<rustkit_net::BlockReason>,
    },
}

struct InsertedSource {
    text: String,
    status: u16,
    mime: Option<String>,
    encoded_size: Option<u64>,
    from_cache: bool,
}

struct InsertedHttp {
    status: u16,
    mime: Option<String>,
    encoded_size: Option<u64>,
    from_cache: bool,
}

struct FetchJob {
    loader: Arc<ResourceLoader>,
    request: Request,
    board: Arc<Mutex<InsertedBoard>>,
    job_id: u64,
    nav_id: u64,
    cancel: Arc<FetchCancel>,
    /// Stamped when it gets one of the process's in-flight permits, when
    /// the Inspector is recording it.
    wait: Option<rustkit_net::policy::SlotWait>,
}

impl InsertedScripts {
    pub(crate) fn new() -> Self {
        Self {
            board: Arc::new(Mutex::new(InsertedBoard {
                nav_id: 0,
                next_id: 1,
                ready_seq: 0,
                jobs: Vec::new(),
                owed: std::collections::VecDeque::new(),
                notify: Arc::new(tokio::sync::Notify::new()),
            })),
        }
    }

    /// Drop every queued script and accept only results for `nav_id` from
    /// now on. A fetch already in flight is cancelled: its future is
    /// dropped, so the socket closes, and its result is not stored.
    pub(crate) fn retire(&self, nav_id: u64) {
        let mut board = lock_board(&self.board);
        cancel_inserted_jobs(&mut board);
        board.nav_id = nav_id;
    }

    /// Drop every queued script without moving the document generation. A
    /// cancelled load uses this: the fetch is cancelled and the result
    /// must not run later.
    pub(crate) fn abandon(&self) {
        let mut board = lock_board(&self.board);
        cancel_inserted_jobs(&mut board);
    }

    fn has_jobs(&self) -> bool {
        let board = lock_board(&self.board);
        !board.jobs.is_empty() || !board.owed.is_empty()
    }

    /// A script is queued, its fetch is still out, or its element is still
    /// owed its event.
    pub(crate) fn pending(&self) -> bool {
        self.has_jobs()
    }

    fn has_ready(&self) -> bool {
        let board = lock_board(&self.board);
        !board.owed.is_empty() || next_ready_index(&board.jobs).is_some()
    }

    fn take_owed(&self) -> Option<OwedEvent> {
        let mut board = lock_board(&self.board);
        board.owed.pop_front()
    }

    fn owe(&self, owed: OwedEvent) {
        lock_board(&self.board).owed.push_back(owed);
    }

    fn take_ready(&self) -> Option<InsertedJob> {
        let mut board = lock_board(&self.board);
        let index = next_ready_index(&board.jobs)?;
        Some(board.jobs.remove(index))
    }

    fn notify(&self) -> Arc<tokio::sync::Notify> {
        lock_board(&self.board).notify.clone()
    }
}

impl Clone for InsertedScripts {
    fn clone(&self) -> Self {
        Self { board: Arc::clone(&self.board) }
    }
}

fn lock_board(board: &Mutex<InsertedBoard>) -> std::sync::MutexGuard<'_, InsertedBoard> {
    board.lock().unwrap_or_else(|e| e.into_inner())
}

/// A finished fetch that must not hold the queue: the element hears `error`
/// and later scripts do not wait for it.
fn does_not_block(body: &FetchedInserted) -> bool {
    match body {
        FetchedInserted::Source(_) => false,
        FetchedInserted::Http(_) | FetchedInserted::Transport { .. } => true,
    }
}

/// The script to run next, if one is ready (HTML §4.12.1.1). An async script
/// (the default for one the page inserted) is ready when its fetch has
/// finished. An in-order script (`async` set false) is ready when its fetch
/// has finished and every in-order script inserted before it has already
/// run, except a failed or blocked fetch: that fires `error` immediately and
/// does not hold the ones after it. Among the scripts that are ready, the
/// one whose fetch finished first runs.
fn next_ready_index(jobs: &[InsertedJob]) -> Option<usize> {
    let mut in_order_blocked = false;
    let mut best: Option<(usize, u64)> = None;
    for (index, job) in jobs.iter().enumerate() {
        let failure = job.body.as_ref().is_some_and(does_not_block);
        if job.in_order && in_order_blocked && !failure {
            continue;
        }
        if job.body.is_none() {
            if job.in_order {
                in_order_blocked = true;
            }
            continue;
        }
        if job.in_order && !failure {
            in_order_blocked = true;
        }
        if best.map(|(_, seq)| job.ready_seq < seq).unwrap_or(true) {
            best = Some((index, job.ready_seq));
        }
    }
    best.map(|(index, _)| index)
}

/// Queue fetches for the external classic scripts the page has inserted
/// since the last take. `false`: the JS engine panicked taking the queue,
/// and the queue has been dropped.
pub(crate) fn enqueue_inserted_scripts(
    bindings: &DomBindings,
    loader: &Arc<ResourceLoader>,
    referrer: &crate::SubresourceReferrer,
    inspector: &Arc<Inspector>,
    scripts: &InsertedScripts,
) -> bool {
    if enqueue_inserted(bindings, loader, referrer, inspector, scripts) {
        return true;
    }
    scripts.abandon();
    false
}

/// Run at most one inserted script whose body has already arrived. Does not
/// wait for a fetch. A script this one inserts is queued before the call
/// returns, so the caller can see that the chain is not idle yet.
pub(crate) fn drive_at_most_one(
    bindings: &DomBindings,
    loader: &Arc<ResourceLoader>,
    referrer: &crate::SubresourceReferrer,
    inspector: &Arc<Inspector>,
    scripts: &InsertedScripts,
    execute: bool,
) -> Pump {
    let mut out = Pump::default();
    if !enqueue_inserted(bindings, loader, referrer, inspector, scripts) {
        out.poisoned = true;
        scripts.abandon();
        return out;
    }
    if !execute || super::script_js_exhausted() || !scripts.has_ready() {
        return out;
    }
    out.rounds += 1;
    if run_inserted(bindings, inspector, scripts, &mut out) {
        scripts.abandon();
        return out;
    }
    if !enqueue_inserted(bindings, loader, referrer, inspector, scripts) {
        out.poisoned = true;
        scripts.abandon();
    }
    out
}

/// Fetch and run the external classic scripts the page inserted
/// (`web_scripts.js`). Each is fetched as a parser-inserted script is (the
/// page's referrer, the script destination the shield classifies by, the
/// loader's cookies), never through page script, and run as its element's
/// script: `currentScript` is the element, then the element hears `load`,
/// or `error` when the fetch failed. One whose `async` was set false waits
/// its turn; every other one runs when its own fetch finishes.
///
/// `deadline` bounds how long this call waits. It does not bound the fetch.
/// A fetch still pending when the deadline passes stays queued, hears
/// neither event yet, and is not recorded over budget. The fetch thread
/// keeps it going. When the body arrives it wakes a live turn, which runs
/// the script. `execute` false only enqueues: a turn with no budget left
/// does not run a script. What ran is in the pump's `records`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_inserted_scripts(
    bindings: &DomBindings,
    loader: Arc<ResourceLoader>,
    referrer: &crate::SubresourceReferrer,
    inspector: &Arc<Inspector>,
    scripts: &InsertedScripts,
    deadline: tokio::time::Instant,
    execute: bool,
) -> Pump {
    let mut out = Pump::default();
    let mut ran = 0u32;
    loop {
        if !enqueue_inserted(bindings, &loader, referrer, inspector, scripts) {
            out.poisoned = true;
            scripts.abandon();
            return out;
        }
        if !scripts.has_jobs() {
            return out;
        }
        // A script whose budget is spent stays queued for a turn that has
        // some; it is never run over budget or dropped.
        if !execute || super::script_js_exhausted() {
            return out;
        }
        if scripts.has_ready() {
            ran += 1;
            out.rounds += 1;
            if run_inserted(bindings, inspector, scripts, &mut out) {
                scripts.abandon();
                return out;
            }
            if ran >= MAX_INSERTED_SCRIPT_ROUNDS {
                // The chain is deeper than one drive follows. Start the next
                // fetch and leave it to the next call; do not drop it.
                if !enqueue_inserted(bindings, &loader, referrer, inspector, scripts) {
                    out.poisoned = true;
                    scripts.abandon();
                }
                return out;
            }
            continue;
        }
        if tokio::time::Instant::now() >= deadline {
            return out;
        }
        // The fetch itself is on the script thread. The event loop polls
        // the load a slice at a time, so this wait does not hold the window.
        let notify = scripts.notify();
        let woke = async move {
            let notified = notify.notified();
            tokio::pin!(notified);
            tokio::select! {
                _ = notified => true,
                _ = tokio::time::sleep_until(deadline) => false,
            }
        }
        .await;
        if !woke {
            return out;
        }
    }
}

/// Queue a fetch for every external classic script the page has inserted
/// since the last take, after the ones already queued. `false`: the JS
/// engine panicked taking the queue.
fn enqueue_inserted(
    bindings: &DomBindings,
    loader: &Arc<ResourceLoader>,
    referrer: &crate::SubresourceReferrer,
    inspector: &Arc<Inspector>,
    scripts: &InsertedScripts,
) -> bool {
    let Ok(queued) = catch_unwind(AssertUnwindSafe(|| bindings.take_inserted_scripts())) else {
        return false;
    };
    if queued.is_empty() {
        return true;
    }
    let mut started = Vec::new();
    let taken_at = std::time::Instant::now();
    {
        let mut board = lock_board(&scripts.board);
        for rustkit_bindings::InsertedScript { node, url, in_order, inserted_by, inserted_at } in queued {
            let queued = taken_at.saturating_duration_since(inserted_at);
            if let Some(at) = inserted_by {
                // The name `run_inserted` evaluates it under.
                inspector.note_inserted(&format!("inserted#{node}"), &at.path, at.line, at.column);
            }
            let id = board.next_id;
            board.next_id = board.next_id.wrapping_add(1);
            let nav_id = board.nav_id;
            // `web_scripts.js` queues only an `href` from the `url` crate's
            // parser and fires `error` for a `src` it cannot resolve, so this
            // always parses (`an_unparseable_inserted_src_never_reaches_the_engine_queue`).
            let Ok(parsed) = Url::parse(&url) else {
                tracing::warn!("inserted script #{node}: queued URL does not parse; skipped");
                debug_assert!(false, "inserted script URL does not parse");
                continue;
            };
            let request = referrer.get_for(parsed.clone(), RequestDestination::Script);
            let guard = inspector
                .begin_request(&parsed, "GET", ResourceType::Script, Some("inserted_script"))
                .map(|g| g.waits_on(&rustkit_net::policy::SlotWait::new()));
            let wait = guard.as_ref().and_then(|g| g.slot()).cloned();
            if let Some(w) = &wait {
                w.queued();
            }
            let cancel = FetchCancel::new();
            board.jobs.push(InsertedJob {
                id,
                node,
                url,
                nav_id,
                in_order,
                ready_seq: 0,
                body: None,
                guard,
                cancel: Arc::clone(&cancel),
                queued,
                requested_at: taken_at,
                arrived_at: None,
                inspector: Arc::clone(inspector),
            });
            started.push(FetchJob {
                loader: Arc::clone(loader),
                request,
                board: Arc::clone(&scripts.board),
                job_id: id,
                nav_id,
                cancel,
                wait,
            });
        }
    }
    for job in started {
        spawn_inserted_fetch(job);
    }
    true
}

/// The fetch thread. One current-thread runtime, so a fetch progresses
/// while the window thread is parked in `ControlFlow::Wait`. The future is
/// built on that thread.
fn spawn_inserted_fetch(job: FetchJob) {
    let tx = fetch_sender();
    let board = Arc::clone(&job.board);
    let job_id = job.job_id;
    let nav_id = job.nav_id;
    if tx.try_send(job).is_err() {
        debug!("inserted-script fetch queue is full or the thread is gone");
        let failed = FetchedInserted::Transport {
            stage: FailureStage::Policy,
            inspector_reason: "too many script requests",
            detail: "inserted-script fetch queue is full".into(),
            blocked_by: None,
        };
        if store_inserted_result(&board, job_id, nav_id, failed) {
            poke_live_turn();
        }
    }
}

fn fetch_sender() -> &'static tokio::sync::mpsc::Sender<FetchJob> {
    static TX: OnceLock<tokio::sync::mpsc::Sender<FetchJob>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<FetchJob>(MAX_INSERTED_FETCH_QUEUE);
        let limit = Arc::new(tokio::sync::Semaphore::new(MAX_INSERTED_FETCHES_IN_FLIGHT));
        std::thread::Builder::new()
            .name("rk-inserted-script".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("inserted-script fetch runtime");
                let local = tokio::task::LocalSet::new();
                rt.block_on(local.run_until(async move {
                    while let Some(job) = rx.recv().await {
                        // A navigation already dropped this job. Do not take
                        // a permit, and do not sit behind the ones in flight.
                        let permit = tokio::select! {
                            biased;
                            _ = job.cancel.cancelled() => continue,
                            permit = limit.clone().acquire_owned() => permit,
                        };
                        let Ok(permit) = permit else {
                            break;
                        };
                        if job.cancel.is_cancelled() {
                            continue;
                        }
                        if let Some(w) = &job.wait {
                            w.granted();
                        }
                        tokio::task::spawn_local(async move {
                            let _permit = permit;
                            run_fetch_job(job).await;
                        });
                    }
                }));
            })
            .expect("inserted-script fetch thread");
        tx
    })
}

async fn run_fetch_job(job: FetchJob) {
    rustkit_js::set_diag_nav(job.nav_id);
    if job.cancel.is_cancelled() {
        log_request(job.request.method.as_str(), job.request.url.as_str(), "-", "cancelled");
        return;
    }
    let fetched = fetch_inserted(&job.loader, job.request, &job.cancel).await;
    if store_inserted_result(&job.board, job.job_id, job.nav_id, fetched) {
        poke_live_turn();
    }
}

/// `true` when the result was stored for a job that is still the current
/// document's. A navigation or a cancel clears the job first, and this
/// stores nothing.
fn store_inserted_result(board: &Mutex<InsertedBoard>, job_id: u64, nav_id: u64, fetched: FetchedInserted) -> bool {
    let mut board = lock_board(board);
    if board.nav_id != nav_id {
        return false;
    }
    let Some(index) = board.jobs.iter().position(|job| job.id == job_id && job.nav_id == nav_id && job.body.is_none()) else {
        return false;
    };
    board.ready_seq = board.ready_seq.wrapping_add(1);
    let ready_seq = board.ready_seq;
    {
        let job = &mut board.jobs[index];
        job.ready_seq = ready_seq;
        job.body = Some(fetched);
        job.arrived_at = Some(std::time::Instant::now());
    }
    let notify = Arc::clone(&board.notify);
    drop(board);
    notify.notify_one();
    true
}

/// One inserted script's HTTP fetch. A 30s stall completes as a network
/// error (the element hears `error` when a turn runs it). This is the
/// request's own timeout, not the turn's budget: the turn stopping its
/// wait does not drop the fetch. A navigation cancels it: the future is
/// dropped and the socket closes.
async fn fetch_inserted(loader: &ResourceLoader, request: Request, cancel: &FetchCancel) -> FetchedInserted {
    if cancel.is_cancelled() {
        return cancelled_fetch();
    }
    let deadline = tokio::time::Instant::now() + crate::LIVE_REQUEST_TIMEOUT;
    let response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return cancelled_fetch(),
        response = tokio::time::timeout_at(deadline, loader.fetch(request)) => response,
    };
    let response = match response {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => {
            let (stage, reason) = inspector::net_error_stage(&e);
            let blocked_by = inspector::net_block_reason(&e).cloned();
            return FetchedInserted::Transport { stage, inspector_reason: reason, detail: e.to_string(), blocked_by };
        }
        Err(_) => {
            return FetchedInserted::Transport {
                stage: FailureStage::Timeout,
                inspector_reason: "timed out",
                detail: "timed out".into(),
                blocked_by: None,
            };
        }
    };
    if !response.ok() {
        return FetchedInserted::Http(InsertedHttp {
            status: response.status.as_u16(),
            mime: response_mime(&response),
            encoded_size: response.encoded_body_size,
            from_cache: response.from_cache,
        });
    }
    let http = InsertedHttp {
        status: response.status.as_u16(),
        mime: response_mime(&response),
        encoded_size: response.encoded_body_size,
        from_cache: response.from_cache,
    };
    let text = tokio::select! {
        biased;
        _ = cancel.cancelled() => return cancelled_fetch(),
        text = tokio::time::timeout_at(deadline, response.text()) => text,
    };
    match text {
        Ok(Ok(text)) => FetchedInserted::Source(InsertedSource {
            text,
            status: http.status,
            mime: http.mime,
            encoded_size: http.encoded_size,
            from_cache: http.from_cache,
        }),
        Ok(Err(e)) => FetchedInserted::Transport {
            stage: FailureStage::Decode,
            inspector_reason: "body is not UTF-8",
            detail: e.to_string(),
            blocked_by: None,
        },
        Err(_) => FetchedInserted::Transport {
            stage: FailureStage::Timeout,
            inspector_reason: "timed out",
            detail: "timed out".into(),
            blocked_by: None,
        },
    }
}

fn response_mime(response: &rustkit_net::Response) -> Option<String> {
    response
        .content_type
        .as_ref()
        .map(|mime| mime.essence_str().to_string())
        .or_else(|| {
            response
                .headers
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
}

/// Fire the oldest owed event, or else run the next inserted script whose
/// body has arrived. Does nothing once the turn's JavaScript budget is
/// spent: the script stays queued for a later turn. `true`: the JS engine
/// panicked and nothing further on this page may run.
fn run_inserted(
    bindings: &DomBindings,
    inspector: &Arc<Inspector>,
    scripts: &InsertedScripts,
    out: &mut Pump,
) -> bool {
    use crate::{EngineFailure, FetchFailure, ScriptOutcome, ScriptRecord};
    if super::script_js_exhausted() {
        return false;
    }
    if let Some(owed) = scripts.take_owed() {
        fire_inserted_event(bindings, owed, out);
        return false;
    }
    let Some(mut job) = scripts.take_ready() else {
        return false;
    };
    let Some(fetched) = job.body.take() else {
        return false;
    };
    finish_inserted_guard(job.guard.take(), &fetched);
    let taken_at = std::time::Instant::now();
    let fetched_in = job.arrived_at.map(|at| at.saturating_duration_since(job.requested_at));
    let waited = job.arrived_at.map(|at| taken_at.saturating_duration_since(at));
    let mut began = false;
    let mut record = ScriptRecord { source: job.url, bytes: 0, elapsed_ms: 0, outcome: ScriptOutcome::Ran };
    let text = match fetched {
        FetchedInserted::Source(source) if source.text.len() > crate::MAX_PAGE_SCRIPT_BYTES => {
            record.bytes = source.text.len();
            Err(ScriptOutcome::Skipped("too large to run inside the script budget"))
        }
        FetchedInserted::Source(source) => Ok(source.text),
        FetchedInserted::Http(http) => Err(ScriptOutcome::FetchFailed(FetchFailure::Status(http.status))),
        FetchedInserted::Transport { detail, .. } => Err(ScriptOutcome::FetchFailed(FetchFailure::Transport(detail))),
    };
    let event = match text {
        Err(outcome) => {
            // Too large is not a network failure, but `error` is still the
            // one event that element gets: a loader waiting on it hears that
            // it will not run.
            let event = match outcome {
                ScriptOutcome::FetchFailed(_) => Some("error"),
                ScriptOutcome::Skipped(reason) if reason.starts_with("too large") => Some("error"),
                _ => None,
            };
            record.outcome = outcome;
            event
        }
        Ok(text) => {
            record.bytes = text.len();
            // Named like a parser-inserted external script, not by its URL
            // (which would become its imports' base).
            let name = format!("inserted#{}", job.node);
            inspector.alias_script(&name, &record.source);
            crate::console_log::alias(&name, &record.source);
            began = true;
            let started = std::time::Instant::now();
            let (ran, stopped) = super::with_script_js(bindings, || {
                let _ = bindings.set_current_script(Some(job.node));
                let ran = catch_unwind(AssertUnwindSafe(|| bindings.evaluate_named(&text, &name)));
                let _ = bindings.set_current_script(None);
                ran
            });
            record.elapsed_ms = started.elapsed().as_millis() as u64;
            record.outcome = if stopped {
                crate::log_script_stopped(&record.source, record.elapsed_ms, crate::take_script_js_overrun());
                ScriptOutcome::OverBudget
            } else {
                match ran {
                    Err(_) => ScriptOutcome::EngineFailed(EngineFailure::Panic),
                    Ok(Ok(_)) => ScriptOutcome::Ran,
                    Ok(Err(e)) => {
                        let message = e.to_string();
                        ScriptOutcome::Threw(
                            message
                                .strip_prefix("JS error: Execution error: ")
                                .map(str::to_string)
                                .unwrap_or(message),
                        )
                    }
                }
            };
            // Run, thrown, or stopped by the budget, the element hears `load`.
            matches!(record.outcome, ScriptOutcome::Ran | ScriptOutcome::Threw(_) | ScriptOutcome::OverBudget)
                .then_some("load")
        }
    };
    if let Some(event) = event {
        crate::console_log::inserted_script(&record.source, event);
    }
    let poisoned = record.outcome == ScriptOutcome::EngineFailed(EngineFailure::Panic);
    let source = record.source.clone();
    let index = out.records.len();
    out.records.push(record);
    out.requests += 1;
    if !poisoned {
        if let Some(event) = event {
            // A spent budget (the script itself may have spent it) owes the
            // event to the next turn that has some, ahead of any other script.
            if super::script_js_exhausted() {
                scripts.owe(OwedEvent { node: job.node, event, source, record: None });
            } else {
                fire_inserted_event(bindings, OwedEvent { node: job.node, event, source, record: Some(index) }, out);
            }
        }
    }
    // After its event: a listener the budget stopped is this script's stop.
    if let Some(record) = out.records.get(index) {
        inspector.script_ended(crate::inspector::ScriptEnd {
            source: &record.source,
            kind: "inserted",
            outcome: &record.outcome,
            started: began,
            bytes: record.bytes,
            queued: Some(job.queued),
            fetch: fetched_in,
            waited,
            ran_ms: record.elapsed_ms,
            event,
        });
    }
    if poisoned {
        out.poisoned = true;
        return true;
    }
    false
}

/// Fire an inserted script's `load` or `error`. A listener the budget
/// stopped is that script's stop in the log; what one threw goes to
/// `out.threw`.
fn fire_inserted_event(bindings: &DomBindings, owed: OwedEvent, out: &mut Pump) {
    let (fired, stopped) = super::with_script_js(bindings, || bindings.fire_script_event(owed.node, owed.event));
    if stopped {
        crate::log_script_stopped(&owed.source, crate::script_js_stopped_ms(), crate::take_script_js_overrun());
        mark_listener_stopped(out, owed.record, owed.source);
    } else if let Err(e) = fired {
        out.threw.push(e.to_string());
    }
}

/// The budget stopped a `load`/`error` listener: that is its script's
/// stop in the log. `record` is the index of the script's own record in
/// this pump; `None` (the event was owed to a later turn) adds one.
pub(crate) fn mark_listener_stopped(out: &mut Pump, record: Option<usize>, source: String) {
    match record.and_then(|index| out.records.get_mut(index)) {
        Some(record) => record.outcome = crate::ScriptOutcome::OverBudget,
        None => out.records.push(crate::ScriptRecord {
            source,
            bytes: 0,
            elapsed_ms: 0,
            outcome: crate::ScriptOutcome::OverBudget,
        }),
    }
}

fn finish_inserted_guard(guard: Option<crate::inspector::RequestGuard>, fetched: &FetchedInserted) {
    let Some(guard) = guard else {
        return;
    };
    match fetched {
        FetchedInserted::Source(source) => guard.response(
            source.status,
            source.mime.as_deref(),
            source.encoded_size,
            Some(source.text.len() as u64),
            source.from_cache,
        ),
        FetchedInserted::Http(http) => {
            guard.response(http.status, http.mime.as_deref(), http.encoded_size, None, http.from_cache);
        }
        FetchedInserted::Transport { stage, inspector_reason, blocked_by, .. } => {
            guard.blocked_by(blocked_by.as_ref()).fail(*stage, inspector_reason)
        }
    }
}
