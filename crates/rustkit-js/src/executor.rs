use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context as TaskContext, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::{Duration, Instant};

use boa_engine::job::{GenericJob, IntervalJob, Job, JobExecutor, NativeAsyncJob, PromiseJob, TimeoutJob};
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue};

fn dummy_waker() -> Waker {
    fn noop(_: *const ()) {}
    fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

/// RAII guard that resets the executor's `context_cell` slot back to its owned
/// placeholder upon exit from `run_jobs`, ensuring caller stack references never
/// outlive the call and that in-flight futures do not hold active borrows.
struct ContextResetGuard {
    cell: *mut RefCell<&'static mut Context>,
    placeholder: *mut Context,
}

impl Drop for ContextResetGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // Avoid double panic / process abort during stack unwinding
            if let Ok(mut slot) = unsafe { &*self.cell }.try_borrow_mut() {
                *slot = unsafe { &mut *self.placeholder };
            }
            return;
        }
        match unsafe { &*self.cell }.try_borrow_mut() {
            Ok(mut slot) => {
                *slot = unsafe { &mut *self.placeholder };
            }
            Err(_) => {
                panic!("ContextResetGuard invariant violated: a future or callback still holds an active borrow on the executor context cell");
            }
        }
    }
}

/// Default maximum number of loop iterations/pumps per `run_jobs` call.
pub const DEFAULT_MAX_JOB_ITERATIONS: u64 = 10_000;

pub(crate) struct HostJobExecutor {
    placeholder_context: *mut Context,
    context_cell: *mut RefCell<&'static mut Context>,
    promise_jobs: RefCell<VecDeque<PromiseJob>>,
    generic_jobs: RefCell<VecDeque<GenericJob>>,
    async_jobs: RefCell<VecDeque<NativeAsyncJob>>,
    timeout_jobs: RefCell<Vec<TimeoutJob>>,
    interval_jobs: RefCell<Vec<IntervalJob>>,
    running_futures: RefCell<Vec<Pin<Box<dyn Future<Output = JsResult<JsValue>>>>>>,
    max_job_iterations: Cell<u64>,
    timeout: Cell<Option<Duration>>,
}

impl Drop for HostJobExecutor {
    fn drop(&mut self) {
        // 1. Clear queued jobs and drop running futures before reclaiming cells
        self.clear();
        // 2. Reclaim the heap-allocated cell and placeholder context (zero per-runtime leak)
        unsafe {
            drop(Box::from_raw(self.context_cell));
            drop(Box::from_raw(self.placeholder_context));
        }
    }
}

impl Default for HostJobExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl HostJobExecutor {
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_JOB_ITERATIONS, None)
    }

    pub fn with_limits(max_job_iterations: u64, timeout: Option<Duration>) -> Self {
        let placeholder = Box::into_raw(Box::new(Context::default()));
        let cell = Box::into_raw(Box::new(RefCell::new(unsafe { &mut *placeholder })));
        Self {
            placeholder_context: placeholder,
            context_cell: cell,
            promise_jobs: RefCell::default(),
            generic_jobs: RefCell::default(),
            async_jobs: RefCell::default(),
            timeout_jobs: RefCell::default(),
            interval_jobs: RefCell::default(),
            running_futures: RefCell::default(),
            max_job_iterations: Cell::new(max_job_iterations),
            timeout: Cell::new(timeout),
        }
    }

    #[inline]
    fn context_ref(&self) -> &'static RefCell<&'static mut Context> {
        unsafe { &*self.context_cell }
    }

    pub fn set_max_job_iterations(&self, max: u64) {
        self.max_job_iterations.set(max);
    }

    #[allow(dead_code)]
    pub fn max_job_iterations(&self) -> u64 {
        self.max_job_iterations.get()
    }

    pub fn set_timeout(&self, timeout: Option<Duration>) {
        self.timeout.set(timeout);
    }

    #[allow(dead_code)]
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout.get()
    }

    /// Purge all queued jobs and running futures. On a runaway loop/timeout breach,
    /// dropping in-flight futures and queues contains the runaway and prevents
    /// further recursive job scheduling.
    pub fn clear(&self) {
        self.promise_jobs.borrow_mut().clear();
        self.generic_jobs.borrow_mut().clear();
        self.async_jobs.borrow_mut().clear();
        self.timeout_jobs.borrow_mut().clear();
        self.interval_jobs.borrow_mut().clear();
        self.running_futures.borrow_mut().clear();
    }
}

impl JobExecutor for HostJobExecutor {
    fn enqueue_job(self: Rc<Self>, job: Job, _context: &mut Context) {
        match job {
            Job::PromiseJob(p) => self.promise_jobs.borrow_mut().push_back(p),
            Job::AsyncJob(a) => self.async_jobs.borrow_mut().push_back(a),
            Job::GenericJob(g) => self.generic_jobs.borrow_mut().push_back(g),
            Job::TimeoutJob(t) => self.timeout_jobs.borrow_mut().push(t),
            Job::IntervalJob(i) => self.interval_jobs.borrow_mut().push(i),
            Job::FinalizationRegistryCleanupJob(fr) => self.async_jobs.borrow_mut().push_back(fr),
            other => {
                // In RustKit, timer scheduling (setInterval / setTimeout) is driven
                // by the host DomBindings event loop (run_timers). Any other unrecognized
                // job variant is logged and ignored.
                let _ = other;
            }
        }
    }

    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        let context_cell = self.context_cell;
        let placeholder = self.placeholder_context;

        // SAFETY: We temporarily stash a mutable reference to `context` into `context_cell`
        // so that async module-load jobs can access it via Boa's job callback API.
        // Aliasing and lifetime safety depend on the following invariants:
        // 1. ALL accesses to Context in run_jobs (including draining promise jobs, generic
        //    jobs, and timeouts) are routed exclusively through `self.context_ref().borrow_mut()`,
        //    ensuring at most one active `&mut Context` at a time.
        // 2. Each `HostJobExecutor` owns its own dedicated heap placeholder Context and cell,
        //    so multiple runtimes/executors never share placeholder or context state.
        // 3. Soundness across intermediate turns relies on `running_futures` being polled strictly
        //    inside `run_jobs` while the caller's stack context is installed in `context_cell`.
        //    Between `run_jobs` calls, futures remain dormant and do not poll or borrow.
        // 4. `ContextResetGuard` verifies via `try_borrow_mut()` that no future or callback holds
        //    an active borrow when `run_jobs` exits, and resets `context_cell` back to this
        //    executor's owned placeholder so the caller's stack borrow never outlives the call.
        *unsafe { &*context_cell }.borrow_mut() = unsafe { std::mem::transmute(&mut *context) };
        let _guard = ContextResetGuard {
            cell: context_cell,
            placeholder,
        };

        let waker = dummy_waker();
        let mut cx = TaskContext::from_waker(&waker);
        let mut first_error: Option<JsError> = None;
        let started = Instant::now();
        let mut iterations: u64 = 0;
        let mut total_jobs: u64 = 0;
        let max_jobs = self.max_job_iterations.get();

        loop {
            iterations += 1;
            if iterations > max_jobs {
                self.clear();
                self.context_ref().borrow_mut().clear_kept_objects();
                return Err(JsError::from(
                    JsNativeError::range().with_message(format!(
                        "Job queue iteration limit ({max_jobs}) exceeded (runaway promise/microtask recursion)"
                    )),
                ));
            }

            if let Some(timeout) = self.timeout.get() {
                if started.elapsed() >= timeout {
                    self.clear();
                    self.context_ref().borrow_mut().clear_kept_objects();
                    return Err(JsError::from(
                        JsNativeError::range().with_message(format!(
                            "Job queue execution timeout ({timeout:?}) exceeded"
                        )),
                    ));
                }
            }

            let mut progress = false;

            // 1. Take any newly queued async jobs and start them
            let new_async = std::mem::take(&mut *self.async_jobs.borrow_mut());
            for job in new_async {
                total_jobs += 1;
                if total_jobs > max_jobs {
                    self.clear();
                    self.context_ref().borrow_mut().clear_kept_objects();
                    return Err(JsError::from(
                        JsNativeError::range().with_message(format!(
                            "Job queue total job limit ({max_jobs}) exceeded"
                        )),
                    ));
                }
                let fut = job.call(self.context_ref());
                self.running_futures.borrow_mut().push(Box::pin(fut));
                progress = true;
            }

            // 2. Poll running futures
            {
                let mut futures = self.running_futures.borrow_mut();
                let mut i = 0;
                while i < futures.len() {
                    match futures[i].as_mut().poll(&mut cx) {
                        Poll::Ready(Ok(_)) => {
                            drop(futures.swap_remove(i));
                            progress = true;
                        }
                        Poll::Ready(Err(e)) => {
                            drop(futures.swap_remove(i));
                            if first_error.is_none() {
                                first_error = Some(e);
                            }
                            progress = true;
                        }
                        Poll::Pending => {
                            i += 1;
                        }
                    }
                }
            }

            // 3. Drain promise jobs through the context slot to avoid aliasing
            let promise_jobs = std::mem::take(&mut *self.promise_jobs.borrow_mut());
            for job in promise_jobs {
                total_jobs += 1;
                if total_jobs > max_jobs {
                    self.clear();
                    self.context_ref().borrow_mut().clear_kept_objects();
                    return Err(JsError::from(
                        JsNativeError::range().with_message(format!(
                            "Job queue total job limit ({max_jobs}) exceeded"
                        )),
                    ));
                }
                if let Err(e) = job.call(&mut *self.context_ref().borrow_mut()) {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
                progress = true;
            }

            // 4. Drain generic jobs through the context slot
            let generic_jobs = std::mem::take(&mut *self.generic_jobs.borrow_mut());
            for job in generic_jobs {
                total_jobs += 1;
                if total_jobs > max_jobs {
                    self.clear();
                    self.context_ref().borrow_mut().clear_kept_objects();
                    return Err(JsError::from(
                        JsNativeError::range().with_message(format!(
                            "Job queue total job limit ({max_jobs}) exceeded"
                        )),
                    ));
                }
                if let Err(e) = job.call(&mut *self.context_ref().borrow_mut()) {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
                progress = true;
            }

            // 5. Drain timeouts through the context slot
            let timeouts = std::mem::take(&mut *self.timeout_jobs.borrow_mut());
            for job in timeouts {
                if !job.cancelled() {
                    total_jobs += 1;
                    if total_jobs > max_jobs {
                        self.clear();
                        self.context_ref().borrow_mut().clear_kept_objects();
                        return Err(JsError::from(
                            JsNativeError::range().with_message(format!(
                                "Job queue total job limit ({max_jobs}) exceeded"
                            )),
                        ));
                    }
                    if let Err(e) = job.call(&mut *self.context_ref().borrow_mut()) {
                        if first_error.is_none() {
                            first_error = Some(e);
                        }
                    }
                    progress = true;
                }
            }

            if !progress {
                break;
            }
        }

        self.context_ref().borrow_mut().clear_kept_objects();
        if let Some(err) = first_error {
            Err(err)
        } else {
            Ok(())
        }
    }
}
