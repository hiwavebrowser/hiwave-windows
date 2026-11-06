/// Represents the limits of different runtime operations.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeLimits {
    /// Max stack size before an error is thrown.
    stack_size: usize,

    /// Max loop iterations before an error is thrown.
    loop_iteration: u64,

    /// Max backtrace count in exception.
    backtrace_limit: usize,

    /// Max JavaScript call-frame recursion.
    recursion: usize,

    /// Max nested host re-entries (`JsObject::call` / `construct` around
    /// `Context::run`). Separate from `recursion`: each re-entry also pushes
    /// a frame, and summing the two made a setter chain die at half the
    /// frame limit. The host cap is the lower one because every re-entry
    /// nests a `Context::run` on the native stack. 1024 of those fit the
    /// main thread (8MB) and the limit throws before that stack overflows.
    /// A 2MB thread overflows near 270 in a debug build, short of this cap;
    /// page script runs on the main thread.
    ///
    /// HIWAVE PATCH (recursion)
    host_recursion: usize,
}

impl Default for RuntimeLimits {
    #[inline]
    fn default() -> Self {
        Self {
            loop_iteration: u64::MAX,
            // Boa's 512 is below a real page: Lit and custom elements build
            // deep, legitimate stacks, and the old check counted each host
            // callback in both counters. 8192 frames covers that. The stack
            // slot limit used to be 10240, which a few hundred frames never
            // reached; it has to grow with the frame cap or a deep call
            // dies as `StackSize` instead.
            recursion: 8192,
            host_recursion: 1024, // HIWAVE PATCH (recursion)
            backtrace_limit: 50,
            stack_size: 64 * 1024,
        }
    }
}

impl RuntimeLimits {
    /// Return the loop iteration limit.
    ///
    /// If the limit is exceeded in a loop it will throw an error.
    ///
    /// The limit value [`u64::MAX`] means that there is no limit.
    #[inline]
    #[must_use]
    pub const fn loop_iteration_limit(&self) -> u64 {
        self.loop_iteration
    }

    /// Set the loop iteration limit.
    ///
    /// If the limit is exceeded in a loop it will throw an error.
    ///
    /// Setting the limit to [`u64::MAX`] means that there is no limit.
    #[inline]
    pub fn set_loop_iteration_limit(&mut self, value: u64) {
        self.loop_iteration = value;
    }

    /// Disable loop iteration limit.
    #[inline]
    pub fn disable_loop_iteration_limit(&mut self) {
        self.loop_iteration = u64::MAX;
    }

    /// Get max backtrace limit for an exception.
    ///
    /// Default is 50.
    #[inline]
    #[must_use]
    pub const fn backtrace_limit(&self) -> usize {
        self.backtrace_limit
    }

    /// Set max backtrace limit for an exception.
    #[inline]
    pub fn set_backtrace_limit(&mut self, value: usize) {
        self.backtrace_limit = value;
    }

    /// Get max stack size.
    #[inline]
    #[must_use]
    pub const fn stack_size_limit(&self) -> usize {
        self.stack_size
    }

    /// Set max stack size before an error is thrown.
    #[inline]
    pub fn set_stack_size_limit(&mut self, value: usize) {
        self.stack_size = value;
    }

    /// Get recursion limit.
    #[inline]
    #[must_use]
    pub const fn recursion_limit(&self) -> usize {
        self.recursion
    }

    /// Set recursion limit before an error is thrown.
    #[inline]
    pub fn set_recursion_limit(&mut self, value: usize) {
        self.recursion = value;
    }

    /// Get the nested host-call limit.
    ///
    /// HIWAVE PATCH (recursion). Default is 1024.
    #[inline]
    #[must_use]
    pub const fn host_recursion_limit(&self) -> usize {
        self.host_recursion
    }

    /// Set the nested host-call limit before an error is thrown.
    ///
    /// HIWAVE PATCH (recursion)
    #[inline]
    pub fn set_host_recursion_limit(&mut self, value: usize) {
        self.host_recursion = value;
    }
}
