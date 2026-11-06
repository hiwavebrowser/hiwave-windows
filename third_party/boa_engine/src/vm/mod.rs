//! Boa's ECMAScript Virtual Machine
//!
//! The Virtual Machine (VM) handles generating instructions, then executing them.
//! This module will provide an instruction set for the AST to use, various traits,
//! plus an interpreter to execute those instructions

use crate::{
    Context, JsError, JsExpect, JsNativeError, JsObject, JsResult, JsString, JsValue, Module,
    builtins::promise::{PromiseCapability, ResolvingFunctions},
    environments::EnvironmentStack,
    error::RuntimeLimitError,
    object::JsFunction,
    realm::Realm,
    script::Script,
    vm::opcode::{OPCODE_HANDLERS, OPCODE_HANDLERS_BUDGET},
    vm::shadow_stack::ErrorStack,
};
use boa_gc::{Finalize, Gc, Trace, custom_trace};
use shadow_stack::ShadowStack;
use std::{cell::Cell, future::Future, ops::ControlFlow, path::Path, pin::Pin, task};

#[cfg(feature = "trace")]
use crate::sys::time::Instant;

#[allow(unused_imports)]
pub(crate) use opcode::{Instruction, InstructionIterator, Opcode};

pub(crate) use {
    call_frame::CallFrameFlags,
    code_block::{
        CodeBlockFlags, Constant, Handler, StopFinally, create_function_object,
        create_function_object_fast,
    },
    completion_record::CompletionRecord,
    inline_cache::InlineCache,
};

pub use runtime_limits::RuntimeLimits;

/// Depth at the most recent recursion check on this thread.
///
/// HIWAVE PATCH (recursion). The interrupt hook has no `Context`, so the
/// check records this where the hook can read it.
#[derive(Clone, Copy, Debug, Default)]
pub struct RecursionSample {
    /// JavaScript frames, excluding the dummy frame.
    pub frames: usize,
    /// Nested `JsObject::call` / `construct` re-entries.
    pub host: usize,
    /// Configured JavaScript frame limit.
    pub recursion_limit: usize,
    /// Configured host re-entry limit.
    pub host_limit: usize,
    /// Deepest `frames` since the last [`Context::reset_recursion_max`].
    pub max_frames: usize,
}

thread_local! {
    static RECURSION_SAMPLE: Cell<RecursionSample> = const { Cell::new(RecursionSample {
        frames: 0,
        host: 0,
        recursion_limit: 0,
        host_limit: 0,
        max_frames: 0,
    }) };
}

/// The last recursion check on this thread.
///
/// HIWAVE PATCH (recursion)
#[must_use]
pub fn recursion_sample() -> RecursionSample {
    RECURSION_SAMPLE.with(|sample| sample.get())
}

pub use {
    call_frame::{CallFrame, GeneratorResumeKind},
    code_block::CodeBlock,
    source_info::{NativeSourceInfo, SourcePath},
};

pub(crate) use code_block::GlobalFunctionBinding;

mod call_frame;
mod code_block;
mod completion_record;
mod inline_cache;
mod runtime_limits;

pub(crate) mod opcode;
pub(crate) mod shadow_stack;
pub(crate) mod source_info;
pub mod interrupt_diag; // HIWAVE PATCH (interrupt diagnostics)
pub mod host_stack; // HIWAVE PATCH (host stack)

#[cfg(feature = "flowgraph")]
pub mod flowgraph;

#[cfg(test)]
mod tests;

/// Virtual Machine.
#[derive(Debug)]
pub struct Vm {
    /// The call frame stack.
    ///
    /// The current frame is always the last element. A dummy frame is always
    /// present at position 0 so the stack is never empty.
    pub(crate) frames: Vec<CallFrame>,

    pub(crate) stack: Stack,

    pub(crate) return_value: JsValue,

    /// When an error is thrown, the pending exception is set.
    ///
    /// If we throw an empty exception ([`None`]), this means that `return()` was called on a generator,
    /// propagating though the exception handlers and executing the finally code (if any).
    ///
    /// See [`ReThrow`](crate::vm::Opcode::ReThrow) and [`ReThrow`](crate::vm::Opcode::Exception) opcodes.
    ///
    /// This eliminates the conversion between [`crate::JsNativeError`] and [`crate::JsValue`] if not needed.
    pub(crate) pending_exception: Option<JsError>,
    pub(crate) runtime_limits: RuntimeLimits,

    // HIWAVE PATCH (interrupt): a word another thread may set to stop the
    // running script, and the turn it is compared with. See PATCHES.md.
    pub(crate) interrupt: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    pub(crate) interrupt_turn: u64,
    // Set by `new_promise_reaction_job` for an engine-internal reaction:
    // the next native call skips the interrupt check once.
    pub(crate) interrupt_exempt_once: bool,
    // Set while the engine settles promises (`Promise::settling`): its own
    // promise bookkeeping is then entered even in a stopped turn.
    pub(crate) settling_promises: bool,
    // END HIWAVE PATCH

    // HIWAVE PATCH (stop finally): how long the `finally` blocks a stop
    // unwinds through may run (`None`: they do not run, as upstream), and
    // the stop unwinding now. See PATCHES.md.
    pub(crate) stop_finally_grace: Option<std::time::Duration>,
    pub(crate) stop_unwind: Option<StopUnwind>,
    // Set while the run entry check reports a stop: a body that has not
    // started in this turn does not run its `finally` either.
    pub(crate) stop_finally_blocked: bool,
    // END HIWAVE PATCH

    /// This is used to assign a native (rust) function as the active function,
    /// because we don't push a frame for them.
    pub(crate) native_active_function: Option<JsObject>,

    /// Number of nested host calls that re-enter the VM via `Context::run()`.
    ///
    /// This is incremented by high-level host entry points such as
    /// [`JsObject::call`](crate::object::JsObject::call) and
    /// [`JsObject::construct`](crate::object::JsObject::construct).
    pub(crate) host_call_depth: usize,

    // HIWAVE PATCH (host stack): names of the functions `call`/`construct`
    // re-entered, parallel to `host_call_depth` while the console log is on.
    // `host_stack_logged` remembers which of 256/512/1000 this climb already
    // reported. See `host_stack`.
    pub(crate) host_frames: Vec<String>,
    pub(crate) host_stack_logged: u8,

    /// Deepest JavaScript frame count (excluding the dummy frame) since
    /// [`Context::reset_recursion_max`](crate::Context::reset_recursion_max).
    ///
    /// HIWAVE PATCH (recursion)
    pub(crate) max_frame_depth: Cell<usize>,

    pub(crate) shadow_stack: ShadowStack,

    #[cfg(feature = "trace")]
    pub(crate) trace: bool,
    #[cfg(feature = "trace")]
    pub(crate) current_frame: Option<*const CallFrame>,
}

// HIWAVE PATCH (stop finally)
/// A stop (the interrupt word, or a runtime limit an embedder uses as one)
/// on its way out of the script, and the `finally` blocks it is running.
#[derive(Debug)]
pub(crate) struct StopUnwind {
    /// What the stop reports once it reaches the Rust caller.
    pub(crate) error: RuntimeLimitError,
    /// No `finally` runs past this, and script still running is stopped.
    pub(crate) until: std::time::Instant,
    /// The `finally` blocks running for the stop, innermost last: the
    /// frame count (`frames.len()`) of the frame each is in, and where.
    pub(crate) running: Vec<(usize, StopFinally)>,
}
// END HIWAVE PATCH

/// The stack holds the [`JsValue`]s for the calling convention and registers.
///
/// The stack is persistent across frames.
/// It's addressing is relative to the frame pointer (`fp`) in each [`CallFrame`].
///
/// The stack stores the following elements:
/// - The function prologue
///   - The `this` value of the function
///   - The function object itself
/// - The arguments of the function
/// - The register file for the frame
/// - Some manually pushed values like the return value of a function.
///
/// ```text
///  Stack: | this | func | arg1 | ... | argN | reg0 | reg1 | ... | regK |
///           ▲                                  ▲
///           └─ fp                              └─ rp
/// ```
#[derive(Clone, Debug, Trace, Finalize)]
pub(crate) struct Stack {
    stack: Vec<JsValue>,
}

impl Stack {
    /// Creates a new stack with the given capacity.
    fn new(capacity: usize) -> Self {
        Self {
            stack: Vec::with_capacity(capacity),
        }
    }

    /// Get a register value by index, relative to the given frame's `rp`.
    pub(crate) fn get_register(&self, frame: &CallFrame, index: usize) -> Option<&JsValue> {
        self.stack.get(frame.rp as usize + index)
    }

    /// Set a register value by index, relative to the given frame's `rp`.
    pub(crate) fn set_register(&mut self, frame: &CallFrame, index: usize, value: JsValue) {
        self.stack[frame.rp as usize + index] = value;
    }

    // HIWAVE PATCH (interrupt): the stack's length, and truncation to it,
    // for `JsObject::call`/`construct` (`object/operations.rs`).
    pub(crate) fn len(&self) -> usize {
        self.stack.len()
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.stack.truncate(len);
    }
    // END HIWAVE PATCH

    /// Truncate the stack to the given frame.
    pub(crate) fn truncate_to_frame(&mut self, frame: &CallFrame) {
        self.stack.truncate(frame.frame_pointer());
    }

    /// Split the stack at the given frame.
    pub(crate) fn split_off_frame(&mut self, frame: &CallFrame) -> Self {
        let frame_pointer = frame.frame_pointer();
        Self {
            stack: self.stack.split_off(frame_pointer),
        }
    }

    /// Get the `this` value of the given frame.
    pub(crate) fn get_this(&self, frame: &CallFrame) -> JsValue {
        self.stack[frame.this_index()].clone()
    }

    /// Set the `this` value of the given frame.
    pub(crate) fn set_this(&mut self, frame: &CallFrame, this: JsValue) {
        self.stack[frame.this_index()] = this;
    }

    /// Get the function object of the given frame.
    pub(crate) fn get_function(&self, frame: &CallFrame) -> Option<JsObject> {
        if let Some(object) = self.stack[frame.function_index()].as_object() {
            return Some(object.clone());
        }
        None
    }

    /// Get the function arguments of the given frame.
    pub(crate) fn get_arguments(&self, frame: &CallFrame) -> &[JsValue] {
        &self.stack[frame.arguments_range()]
    }

    /// Get a single function argument of the given frame by index.
    pub(crate) fn get_argument(&self, frame: &CallFrame, index: usize) -> Option<&JsValue> {
        self.get_arguments(frame).get(index)
    }

    /// Get the rest arguments of the given frame.
    pub(crate) fn pop_rest_arguments(&mut self, frame: &CallFrame) -> Option<Vec<JsValue>> {
        let argument_count = frame.argument_count as usize;
        let param_count = frame.code_block().parameter_length as usize;
        if argument_count < param_count {
            return None;
        }
        let args_start = frame.fp as usize + CallFrame::FUNCTION_PROLOGUE as usize;
        let args_end = args_start + argument_count;
        let rest_count = argument_count - param_count + 1;

        Some(
            self.stack
                .drain((args_end - rest_count)..args_end)
                .collect(),
        )
    }

    /// Push a value on the stack.
    pub(crate) fn push<T>(&mut self, value: T)
    where
        T: Into<JsValue>,
    {
        self.stack.push(value.into());
    }

    /// Pop a value off the stack.
    ///
    /// # Panics
    ///
    /// If there is nothing to pop, then this will panic.
    #[track_caller]
    pub(crate) fn pop(&mut self) -> JsValue {
        self.stack.pop().expect("stack was empty")
    }

    /// Pop the function arguments according to the calling convention.
    /// This will pop the last `argument_count` values from the stack.
    pub(crate) fn calling_convention_pop_arguments(
        &mut self,
        argument_count: usize,
    ) -> Vec<JsValue> {
        let index = self.stack.len() - argument_count;
        self.stack.split_off(index)
    }

    /// Push the function arguments according to the calling convention.
    /// This will push the given values onto the stack.
    pub(crate) fn calling_convention_push_arguments(&mut self, values: &[JsValue]) {
        self.stack.extend_from_slice(values);
    }

    /// Get the function object at the top of the stack according to the calling convention.
    #[track_caller]
    pub(crate) fn calling_convention_get_function(&self, argument_count: usize) -> &JsValue {
        let index = self.stack.len() - 1 - argument_count;
        self.stack
            .get(index)
            .expect("invalid calling convention function index")
    }

    /// Set the function object value at the top of the stack according to the calling convention.
    #[track_caller]
    pub(crate) fn calling_convention_set_function(
        &mut self,
        argument_count: usize,
        function: JsValue,
    ) {
        let index = self.stack.len() - 1 - argument_count;
        self.stack[index] = function;
    }

    /// Set the `this` value at the top of the stack according to the calling convention.
    #[track_caller]
    pub(crate) fn calling_convention_set_this(&mut self, argument_count: usize, function: JsValue) {
        let index = self.stack.len() - 2 - argument_count;
        self.stack[index] = function;
    }

    /// Insert the function arguments at the top of the stack according to the calling convention.
    /// This will insert the given values at the position of the function arguments.
    pub(crate) fn calling_convention_insert_arguments(
        &mut self,
        existing_argument_count: usize,
        arguments: &[JsValue],
    ) {
        let index = self.stack.len() - existing_argument_count;
        self.stack.splice(index..index, arguments.iter().cloned());
    }

    #[cfg(feature = "trace")]
    const MAX_VALUE_LEN: usize = 18;
    #[cfg(feature = "trace")]
    const MAX_STACK_WIDTH: usize = 68;

    #[cfg(feature = "trace")]
    fn raw_value(value: &JsValue) -> String {
        match value {
            v if v.is_callable() => "func".to_string(),
            v if v.is_object() => "obj".to_string(),
            v if v.is_undefined() => "und".to_string(),
            v if v.is_null() => "null".to_string(),
            v => v.display().to_string(),
        }
    }

    #[cfg(feature = "trace")]
    fn truncate_display(val: &str) -> String {
        if val.len() <= Self::MAX_VALUE_LEN {
            return val.to_string();
        }
        let mut end = Self::MAX_VALUE_LEN - 2;
        while !val.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}..", &val[..end])
    }

    #[cfg(feature = "trace")]
    fn display_trace(&self, frame: &CallFrame, frame_count: usize) -> String {
        let total = self.stack.len();
        if total == 0 {
            return "[ <empty> ]".to_string();
        }

        let mut groups: Vec<(String, usize, Option<usize>)> = Vec::new();
        let mut force_truncate = false;

        // Lazily group values to avoid eagerly evaluating `raw_value` for the entire stack.
        for (idx, v) in self.stack.iter().enumerate().rev() {
            let is_frame = frame.frame_pointer() == idx;
            let raw = Self::raw_value(v);

            if !is_frame
                && let Some(last) = groups.last_mut()
                && last.0 == raw
                && last.2.is_none()
            {
                last.1 += 1;
            } else {
                let marker = if is_frame { Some(frame_count) } else { None };
                groups.push((raw, 1, marker));

                // If groups is large enough to mathematically guarantee overflowing the display width,
                // we can stop evaluating to save instruction budget / time.
                if groups.len() > Self::MAX_STACK_WIDTH / 2 {
                    force_truncate = true;
                    break;
                }
            }
        }

        let mut string = String::from("[ ");
        let mut truncated = force_truncate;
        let suffix = format!(".. ({total} total) ]");

        for (i, (val, count, marker)) in groups.iter().enumerate() {
            let display_val = Self::truncate_display(val);
            let part = if *count > 1 {
                format!("{display_val} (x{count})")
            } else {
                display_val
            };

            let separator = if let Some(fc) = marker {
                format!(" |{fc}|")
            } else if i + 1 < groups.len() {
                ",".to_string()
            } else {
                String::new()
            };

            let addition = format!("{part}{separator} ");
            if string.len() + addition.len() + suffix.len() > Self::MAX_STACK_WIDTH {
                truncated = true;
                break;
            }
            string.push_str(&addition);
        }

        if truncated {
            string.push_str(&suffix);
        } else {
            string.push(']');
        }
        string
    }
}

/// Active runnable in the current vm context.
#[derive(Debug, Clone, Finalize)]
pub enum ActiveRunnable {
    /// A [**Script Record**](https://tc39.es/ecma262/#sec-script-records)
    Script(Script),
    /// A [**Source Text Module Record**](https://tc39.es/ecma262/#sec-source-text-module-records).
    Module(Module),
}

unsafe impl Trace for ActiveRunnable {
    custom_trace!(this, mark, {
        match this {
            Self::Script(script) => mark(script),
            Self::Module(module) => mark(module),
        }
    });
}

impl ActiveRunnable {
    /// Gets the path of the runnable, if it has one.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Script(script) => script.path(),
            Self::Module(module) => module.path(),
        }
    }
}

impl Vm {
    /// Creates a new virtual machine.
    pub(crate) fn new(realm: Realm) -> Self {
        let mut frames = Vec::with_capacity(16);
        frames.push(CallFrame::new(
            Gc::new(CodeBlock::new(JsString::default(), 0, true)),
            None,
            EnvironmentStack::new(),
            realm,
        ));
        Self {
            frames,
            stack: Stack::new(1024),
            return_value: JsValue::undefined(),
            pending_exception: None,
            runtime_limits: RuntimeLimits::default(),
            interrupt: None,              // HIWAVE PATCH (interrupt)
            interrupt_turn: 0,            // HIWAVE PATCH (interrupt)
            interrupt_exempt_once: false, // HIWAVE PATCH (interrupt)
            settling_promises: false,     // HIWAVE PATCH (interrupt)
            stop_finally_grace: None,     // HIWAVE PATCH (stop finally)
            stop_unwind: None,            // HIWAVE PATCH (stop finally)
            stop_finally_blocked: false,  // HIWAVE PATCH (stop finally)
            native_active_function: None,
            host_call_depth: 0,
            host_frames: Vec::new(),      // HIWAVE PATCH (host stack)
            host_stack_logged: 0,         // HIWAVE PATCH (host stack)
            max_frame_depth: Cell::new(0), // HIWAVE PATCH (recursion)
            shadow_stack: ShadowStack::default(),
            #[cfg(feature = "trace")]
            trace: false,
            #[cfg(feature = "trace")]
            current_frame: None,
        }
    }

    #[track_caller]
    #[inline]
    pub(crate) fn set_register(&mut self, index: usize, value: JsValue) {
        let rp = self.frame().rp as usize;
        debug_assert!(
            rp + index < self.stack.stack.len(),
            "register index out of bounds: rp {rp}, index {index}, stack len {}",
            self.stack.stack.len()
        );
        // SAFETY: Register indices are determined by the bytecode compiler and are
        // guaranteed to be within the register bounds for well-formed bytecode. The
        // debug_assert above catches any compiler bugs during development.
        unsafe {
            *self.stack.stack.get_unchecked_mut(rp + index) = value;
        }
    }

    #[track_caller]
    #[inline]
    pub(crate) fn get_register(&self, index: usize) -> &JsValue {
        let rp = self.frame().rp as usize;
        debug_assert!(
            rp + index < self.stack.stack.len(),
            "register index out of bounds: rp {rp}, index {index}, stack len {}",
            self.stack.stack.len()
        );
        // SAFETY: Register indices are determined by the bytecode compiler and are
        // guaranteed to be within the register bounds for well-formed bytecode. The
        // debug_assert above catches any compiler bugs during development.
        unsafe { self.stack.stack.get_unchecked(rp + index) }
    }

    /// Takes the value from a register, replacing it with `undefined`.
    ///
    /// Use this instead of `get_register().clone()` when the register value is
    /// consumed and won't be read again, to avoid unnecessary Gc refcount increments.
    #[track_caller]
    #[inline]
    pub(crate) fn take_register(&mut self, index: usize) -> JsValue {
        let rp = self.frame().rp as usize;
        debug_assert!(
            rp + index < self.stack.stack.len(),
            "register index out of bounds: rp {rp}, index {index}, stack len {}",
            self.stack.stack.len()
        );
        // SAFETY: Register indices are determined by the bytecode compiler and are
        // guaranteed to be within the register bounds for well-formed bytecode. The
        // debug_assert above catches any compiler bugs during development.
        unsafe { std::mem::take(self.stack.stack.get_unchecked_mut(rp + index)) }
    }

    /// Set the promise capability for the current frame.
    #[track_caller]
    pub(crate) fn set_promise_capability(
        &mut self,
        promise_capability: PromiseCapability,
    ) -> JsResult<()> {
        #[cfg(debug_assertions)]
        {
            if !self.frame().code_block().is_async() {
                return Err(crate::error::PanicError::new(
                    "only async functions and modules with a top-level-await \
                    can have a promise capability",
                )
                .into());
            }
        }

        let rp = self.frame().rp as usize;
        self.stack.stack[rp + CallFrame::PROMISE_CAPABILITY_PROMISE_REGISTER_INDEX] =
            promise_capability.promise.into();
        self.stack.stack[rp + CallFrame::PROMISE_CAPABILITY_RESOLVE_REGISTER_INDEX] =
            promise_capability.functions.resolve.into();
        self.stack.stack[rp + CallFrame::PROMISE_CAPABILITY_REJECT_REGISTER_INDEX] =
            promise_capability.functions.reject.into();

        Ok(())
    }

    /// Get the promise capability for the current frame.
    #[track_caller]
    pub(crate) fn get_promise_capability(&self) -> JsResult<PromiseCapability> {
        #[cfg(debug_assertions)]
        if !self.frame().code_block().is_async() {
            return Err(crate::error::PanicError::new(
                "cannot get promise capability from non-async code",
            )
            .into());
        }

        let rp = self.frame().rp as usize;
        let promise = self
            .stack
            .stack
            .get(rp + CallFrame::PROMISE_CAPABILITY_PROMISE_REGISTER_INDEX)
            .and_then(JsValue::as_object)
            .js_expect("registers must have a promise capability")?;
        let resolve = self
            .stack
            .stack
            .get(rp + CallFrame::PROMISE_CAPABILITY_RESOLVE_REGISTER_INDEX)
            .and_then(JsValue::as_object)
            .and_then(JsFunction::from_object)
            .js_expect("registers must have a resolve function")?;
        let reject = self
            .stack
            .stack
            .get(rp + CallFrame::PROMISE_CAPABILITY_REJECT_REGISTER_INDEX)
            .and_then(JsValue::as_object)
            .and_then(JsFunction::from_object)
            .js_expect("registers must have a reject function")?;

        Ok(PromiseCapability {
            promise,
            functions: ResolvingFunctions { resolve, reject },
        })
    }

    /// Get the async generator object for the current frame.
    #[track_caller]
    pub(crate) fn async_generator_object(&self) -> Option<JsObject> {
        if !self.frame().code_block().is_async_generator() {
            return None;
        }

        let rp = self.frame().rp as usize;
        self.stack
            .stack
            .get(rp + CallFrame::ASYNC_GENERATOR_OBJECT_REGISTER_INDEX)
            .expect("registers must have an async generator object")
            .as_object()
    }

    /// Retrieves the VM frame.
    ///
    /// NOTE: When you need a `&CallFrame` alongside a mutable borrow of another
    /// `Vm` field (e.g. `stack`), use `self.vm.frames.last().expect("frame must exist")` instead
    /// so that the borrow checker can split the borrows.
    #[track_caller]
    #[inline]
    pub(crate) fn frame(&self) -> &CallFrame {
        // SAFETY: `frames` always contains at least the dummy frame.
        unsafe { self.frames.last().unwrap_unchecked() }
    }

    /// Retrieves the VM frame mutably.
    ///
    /// NOTE: When you need a `&mut CallFrame` alongside a mutable borrow of another
    /// `Vm` field (e.g. `stack`), use `self.vm.frames.last_mut().expect("frame must exist")` instead
    /// so that the borrow checker can split the borrows.
    #[track_caller]
    #[inline]
    pub(crate) fn frame_mut(&mut self) -> &mut CallFrame {
        // SAFETY: `frames` always contains at least the dummy frame.
        unsafe { self.frames.last_mut().unwrap_unchecked() }
    }

    pub(crate) fn push_frame(&mut self, mut frame: CallFrame) {
        // Each function call starts with an implicit `undefined` return value.
        self.return_value = JsValue::undefined();

        // NOTE: We need to check if we already pushed the registers,
        //       since generator-like functions push the same call
        //       frame with pre-built stack and registers (fp and rp already set).
        if !frame.registers_already_pushed() {
            let current_stack_length = self.stack.stack.len() as u32;
            frame.fp = current_stack_length - frame.argument_count - CallFrame::FUNCTION_PROLOGUE;

            let register_count = frame.code_block.register_count as usize;
            frame.rp = self.stack.stack.len() as u32;
            self.stack.stack.resize(
                self.stack.stack.len() + register_count,
                JsValue::undefined(),
            );
        }

        // Keep carrying the last active runnable in case the current callframe
        // yields.
        if frame.active_runnable.is_none() {
            let current = self.frame();
            frame.active_runnable.clone_from(&current.active_runnable);
        }

        let current_pc = self.frame().pc;
        self.shadow_stack
            .push_bytecode(current_pc, frame.code_block().source_info.clone());

        self.frames.push(frame);
    }

    pub(crate) fn push_frame_with_stack(
        &mut self,
        frame: CallFrame,
        this: JsValue,
        function: JsValue,
    ) {
        self.stack.push(this);
        self.stack.push(function);

        self.push_frame(frame);
    }

    pub(crate) fn pop_frame(&mut self) -> Option<CallFrame> {
        // Don't pop the dummy frame (index 0).
        if self.frames.len() <= 1 {
            return None;
        }
        self.shadow_stack.pop();
        self.frames.pop()
    }

    /// Handles an exception thrown at position `pc`.
    ///
    /// Returns `true` if the exception was handled, `false` otherwise.
    #[inline]
    pub(crate) fn handle_exception_at(&mut self, pc: u32) -> bool {
        let frame = self.frame_mut();
        let Some((_, handler)) = frame.code_block().find_handler(pc) else {
            return false;
        };

        let catch_address = handler.handler();
        let environment_sp = frame.env_fp + handler.environment_count;

        // Go to handler location.
        frame.pc = u32::from(catch_address);

        self.frame_mut()
            .environments
            .truncate(environment_sp as usize);

        true
    }

    pub(crate) fn get_return_value(&self) -> JsValue {
        self.return_value.clone()
    }

    pub(crate) fn set_return_value(&mut self, value: JsValue) {
        self.return_value = value;
    }

    pub(crate) fn take_return_value(&mut self) -> JsValue {
        std::mem::take(&mut self.return_value)
    }
}

#[allow(clippy::print_stdout)]
#[cfg(feature = "trace")]
impl Context {
    const COLUMN_WIDTH: usize = 26;
    const TIME_COLUMN_WIDTH: usize = Self::COLUMN_WIDTH / 2;
    const OPCODE_COLUMN_WIDTH: usize = Self::COLUMN_WIDTH;
    const OPERAND_COLUMN_WIDTH: usize = Self::COLUMN_WIDTH;
    const NUMBER_OF_COLUMNS: usize = 4;

    pub(crate) fn trace_call_frame(&self) {
        let frame = self.vm.frame();
        let msg = if self.vm.frames.is_empty() {
            " VM Start ".to_string()
        } else {
            format!(
                " Call Frame '{}'{} ",
                frame.code_block().name().to_std_string_escaped(),
                if frame.code_block().name().is_empty() {
                    format!(" [anon#{}]", frame.code_block().debug_id)
                } else {
                    String::new()
                }
            )
        };

        // Only print a functions compiled output if it has not been printed already
        if !frame.code_block.traced.get() {
            println!("{}", frame.code_block);
            frame.code_block.traced.set(true);
        }
        println!(
            "{msg:-^width$}",
            width = Self::COLUMN_WIDTH * Self::NUMBER_OF_COLUMNS - 10
        );
        println!(
            "{:<TIME_COLUMN_WIDTH$} {:<OPCODE_COLUMN_WIDTH$} {:<OPERAND_COLUMN_WIDTH$} Stack\n",
            "Time",
            "Opcode",
            "Operands",
            TIME_COLUMN_WIDTH = Self::TIME_COLUMN_WIDTH,
            OPCODE_COLUMN_WIDTH = Self::OPCODE_COLUMN_WIDTH,
            OPERAND_COLUMN_WIDTH = Self::OPERAND_COLUMN_WIDTH,
        );
    }

    fn trace_execute_instruction<F>(
        &mut self,
        f: F,
        opcode: Opcode,
    ) -> ControlFlow<CompletionRecord>
    where
        F: FnOnce(&mut Context, Opcode) -> ControlFlow<CompletionRecord>,
    {
        if self.vm.current_frame != Some(self.vm.frame()) {
            println!();
            self.trace_call_frame();
            self.vm.current_frame = Some(self.vm.frame());
        }
        let frame = self.vm.frame();
        let (instruction, _) = frame
            .code_block
            .bytecode
            .next_instruction(frame.pc as usize);
        let operands = self
            .vm
            .frame()
            .code_block()
            .instruction_operands(&instruction);

        let instant = Instant::now();
        let result = self.execute_instruction(f, opcode);
        let duration = instant.elapsed();

        let stack = self
            .vm
            .stack
            .display_trace(self.vm.frame(), self.vm.frames.len() - 1);

        println!(
            "{:<TIME_COLUMN_WIDTH$} {:<OPCODE_COLUMN_WIDTH$} {operands:<OPERAND_COLUMN_WIDTH$} {stack}",
            format!("{}μs", duration.as_micros()),
            opcode.as_str().to_string(),
            TIME_COLUMN_WIDTH = Self::TIME_COLUMN_WIDTH,
            OPCODE_COLUMN_WIDTH = Self::OPCODE_COLUMN_WIDTH,
            OPERAND_COLUMN_WIDTH = Self::OPERAND_COLUMN_WIDTH,
        );

        result
    }
}

impl Context {
    fn execute_instruction<F>(&mut self, f: F, opcode: Opcode) -> ControlFlow<CompletionRecord>
    where
        F: FnOnce(&mut Context, Opcode) -> ControlFlow<CompletionRecord>,
    {
        f(self, opcode)
    }

    fn execute_one<F>(&mut self, f: F, opcode: Opcode) -> ControlFlow<CompletionRecord>
    where
        F: FnOnce(&mut Context, Opcode) -> ControlFlow<CompletionRecord>,
    {
        #[cfg(feature = "fuzz")]
        {
            use crate::error::EngineError;
            if self.instructions_remaining == 0 {
                return ControlFlow::Break(CompletionRecord::Throw(
                    EngineError::NoInstructionsRemain.into(),
                ));
            }
            self.instructions_remaining -= 1;
        }

        #[cfg(feature = "trace")]
        if self.vm.trace || self.vm.frame().code_block.traceable() {
            self.trace_execute_instruction(f, opcode)
        } else {
            self.execute_instruction(f, opcode)
        }

        #[cfg(not(feature = "trace"))]
        self.execute_instruction(f, opcode)
    }

    fn handle_error(&mut self, mut err: JsError) -> ControlFlow<CompletionRecord> {
        // Capture the backtrace early, before any exception handler check,
        // so that errors caught by internal handlers (e.g. async module
        // evaluation) still carry source position information.
        if err.backtrace.is_none() {
            err.backtrace = Some(
                self.vm
                    .shadow_stack
                    .take(self.vm.runtime_limits.backtrace_limit(), self.vm.frame().pc),
            );
        }

        // If we hit the execution step limit, bubble up the error to the
        // (Rust) caller instead of trying to handle as an exception.
        if !err.is_catchable() {
            // HIWAVE PATCH (stop finally)
            if let Some(flow) = self.enter_stop_finally(&err) {
                return flow;
            }
            // END HIWAVE PATCH
            let mut frame = None;
            let mut env_fp = self.vm.frame().environments.len();
            loop {
                if self.vm.frame().exit_early() {
                    break;
                }

                env_fp = self.vm.frame().env_fp as usize;

                let Some(f) = self.vm.pop_frame() else {
                    break;
                };
                frame = Some(f);
            }
            self.vm.frame_mut().environments.truncate(env_fp);
            if let Some(frame) = frame {
                self.vm.stack.truncate_to_frame(&frame);
            }
            // HIWAVE PATCH (interrupt): also drop the exit-early frame's own
            // values, as `handle_throw` does. Its caller pops the frame
            // without truncating, so every stopped turn left them behind
            // until the stack limit failed every later script.
            if self.vm.frame().exit_early() {
                let fp = self.vm.frame().frame_pointer();
                self.vm.stack.stack.truncate(fp);
            }
            // END HIWAVE PATCH
            // HIWAVE PATCH (stop finally): out of the script the Rust caller
            // started (a script, a job): the next one is stopped afresh.
            if self.vm.frames.len() <= 2 {
                self.vm.stop_unwind = None;
            }
            // END HIWAVE PATCH
            return ControlFlow::Break(CompletionRecord::Throw(err));
        }

        // HIWAVE PATCH (stop finally)
        if self.stop_claims_throw() {
            return self.resume_stop();
        }
        // END HIWAVE PATCH

        if let Some(native) = err.as_native_mut()
            && let ErrorStack::Position(position) = &mut native.stack.0
        {
            let backtrace = self.vm.shadow_stack.take_and_push(
                self.vm.runtime_limits.backtrace_limit(),
                self.vm.frame().pc,
                position.clone(),
            );
            native.stack.0 = ErrorStack::Backtrace(backtrace);
        }

        // Note: -1 because we increment after fetching the opcode.
        let pc = self.vm.frame().pc.saturating_sub(1);
        if self.vm.handle_exception_at(pc) {
            self.vm.pending_exception = Some(err);
            return ControlFlow::Continue(());
        }

        // Inject realm before crossing the function boundary
        let err = err.inject_realm(self.realm().clone());

        self.vm.pending_exception = Some(err);
        self.handle_throw()
    }

    fn handle_return(&mut self) -> ControlFlow<CompletionRecord> {
        // HIWAVE PATCH (stop finally): a `return` in a `finally` the stop
        // runs does not end the stop.
        if self.in_stop_finally_frame() {
            return self.resume_stop();
        }
        // END HIWAVE PATCH
        let exit_early = self.vm.frame().exit_early();
        // HIWAVE PATCH (stop finally): the script the Rust caller started
        // ended normally (an `await` or `yield` in a `finally` the stop ran
        // suspended it): the stop is over, and the next script is not
        // stopped by what is left of its grace.
        if exit_early && self.vm.frames.len() <= 2 {
            self.vm.stop_unwind = None;
        }
        // END HIWAVE PATCH
        let frame = self.vm.frames.last().expect("frame must exist");
        self.vm.stack.truncate_to_frame(frame);

        let result = self.vm.take_return_value();
        if exit_early {
            return ControlFlow::Break(CompletionRecord::Return(result));
        }

        self.vm.stack.push(result);
        self.vm.pop_frame().expect("frame must exist");
        ControlFlow::Continue(())
    }

    fn handle_yield(&mut self) -> ControlFlow<CompletionRecord> {
        let result = self.vm.take_return_value();
        // HIWAVE PATCH (stop finally): as `handle_return`.
        if self.vm.frame().exit_early() && self.vm.frames.len() <= 2 {
            self.vm.stop_unwind = None;
        }
        // END HIWAVE PATCH
        if self.vm.frame().exit_early() {
            return ControlFlow::Break(CompletionRecord::Normal(result));
        }

        self.vm.stack.push(result);
        self.vm.pop_frame().expect("frame must exist");
        ControlFlow::Continue(())
    }

    fn handle_throw(&mut self) -> ControlFlow<CompletionRecord> {
        if self
            .vm
            .pending_exception
            .as_ref()
            .is_some_and(|err| err.backtrace.is_none())
        {
            let pc = self.vm.frames.last().expect("frame must exist").pc;
            let limit = self.vm.runtime_limits.backtrace_limit();
            let backtrace = self.vm.shadow_stack.take(limit, pc);
            self.vm
                .pending_exception
                .as_mut()
                .expect("pending exception must exist")
                .backtrace = Some(backtrace);
        }

        let mut env_fp = self.vm.frame().env_fp;
        if self.vm.frame().exit_early() {
            self.vm.frame_mut().environments.truncate(env_fp as usize);
            let frame = self.vm.frames.last().expect("frame must exist");
            self.vm.stack.truncate_to_frame(frame);
            return ControlFlow::Break(CompletionRecord::Throw(
                self.vm
                    .pending_exception
                    .take()
                    .expect("Err must exist for a CompletionType::Throw"),
            ));
        }

        let mut frame = self.vm.pop_frame().expect("frame must exist");

        loop {
            env_fp = self.vm.frame().env_fp;
            let pc = self.vm.frame().pc;
            let exit_early = self.vm.frame().exit_early();

            if self.vm.handle_exception_at(pc) {
                return ControlFlow::Continue(());
            }

            if exit_early {
                return ControlFlow::Break(CompletionRecord::Throw(
                    self.vm
                        .pending_exception
                        .take()
                        .expect("Err must exist for a CompletionType::Throw"),
                ));
            }

            let Some(f) = self.vm.pop_frame() else {
                break;
            };
            frame = f;
        }
        self.vm.frame_mut().environments.truncate(env_fp as usize);
        self.vm.stack.truncate_to_frame(&frame);
        ControlFlow::Continue(())
    }

    /// Runs the current frame to completion, yielding to the caller each time `budget`
    /// "clock cycles" have passed.
    #[allow(clippy::future_not_send)]
    pub(crate) async fn run_async_with_budget(&mut self, budget: u32) -> CompletionRecord {
        let mut runtime_budget: u32 = budget;

        while let Some(byte) = self
            .vm
            .frame()
            .code_block
            .bytecode
            .bytes
            .get(self.vm.frame().pc as usize)
        {
            let opcode = Opcode::decode(*byte);

            match self.execute_one(
                |context, opcode| {
                    let frame = context.vm.frame();
                    let pc = frame.pc as usize;

                    OPCODE_HANDLERS_BUDGET[opcode as usize](context, pc, &mut runtime_budget)
                },
                opcode,
            ) {
                ControlFlow::Continue(()) => {}
                ControlFlow::Break(value) => return value,
            }

            if runtime_budget == 0 {
                runtime_budget = budget;
                yield_now().await;
            }
        }

        CompletionRecord::Throw(JsError::from_native(JsNativeError::error()))
    }

    pub(crate) fn run(&mut self) -> CompletionRecord {
        // HIWAVE PATCH (interrupt): no bytecode runs in a stopped turn, even
        // when an engine reaction let in by `new_promise_reaction_job`
        // resumes a body or runs a module: it fails as its first opcode
        // would.
        if let Err(err) = self.check_interrupt() {
            // HIWAVE PATCH (stop finally): a body that has not started does
            // not run its `finally`.
            self.vm.stop_finally_blocked = true;
            let flow = self.handle_error(err);
            self.vm.stop_finally_blocked = false;
            if let ControlFlow::Break(value) = flow {
                return value;
            }
        }
        // END HIWAVE PATCH
        while let Some(byte) = self
            .vm
            .frame()
            .code_block
            .bytecode
            .bytes
            .get(self.vm.frame().pc as usize)
        {
            let opcode = Opcode::decode(*byte);

            match self.execute_one(
                |context, opcode| {
                    let frame = context.vm.frame();
                    let pc = frame.pc as usize;

                    OPCODE_HANDLERS[opcode as usize](context, pc)
                },
                opcode,
            ) {
                ControlFlow::Continue(()) => {}
                ControlFlow::Break(value) => return value,
            }
        }

        CompletionRecord::Throw(JsError::from_native(JsNativeError::error()))
    }

    /// Checks if we haven't exceeded the defined runtime limits.
    pub(crate) fn check_runtime_limits(&self) -> JsResult<()> {
        // Sample before the interrupt check: an `Interrupted` stop has no
        // `Context` by the time it is logged, and the depth is already known.
        self.observe_recursion(); // HIWAVE PATCH (recursion)
        self.check_interrupt()?; // HIWAVE PATCH (interrupt)
        self.enforce_depth_limits() // HIWAVE PATCH (recursion)
    }

    // HIWAVE PATCH (interrupt): the recursion and stack checks of
    // `check_runtime_limits`, split out so a native call can make them
    // without the interrupt check (`interrupt_exempt_once`).
    pub(crate) fn check_depth_limits(&self) -> JsResult<()> {
        self.observe_recursion(); // HIWAVE PATCH (recursion)
        self.enforce_depth_limits()
    }

    // HIWAVE PATCH (recursion): frame count and host re-entry are separate
    // limits. Summing them counted every setter, getter and host callback
    // twice (the frame `function_call` is about to push, plus
    // `host_call_depth`, which `JsObject::call` increments around
    // `Context::run`). `<=` matches the historical check, so a limit of 10
    // still rejects `factorial(11)` and accepts `factorial(8)`.
    fn enforce_depth_limits(&self) -> JsResult<()> {
        let frames = self.vm.frames.len().saturating_sub(1);
        if self.vm.runtime_limits.recursion_limit() <= frames
            || self.vm.runtime_limits.host_recursion_limit() <= self.vm.host_call_depth
        {
            return Err(RuntimeLimitError::Recursion.into());
        }
        if self.vm.runtime_limits.stack_size_limit() <= self.vm.stack.stack.len() {
            return Err(RuntimeLimitError::StackSize.into());
        }
        Ok(())
    }

    fn observe_recursion(&self) {
        let frames = self.vm.frames.len().saturating_sub(1);
        let host = self.vm.host_call_depth;
        let max = self.vm.max_frame_depth.get().max(frames);
        self.vm.max_frame_depth.set(max);
        RECURSION_SAMPLE.with(|sample| {
            sample.set(RecursionSample {
                frames,
                host,
                recursion_limit: self.vm.runtime_limits.recursion_limit(),
                host_limit: self.vm.runtime_limits.host_recursion_limit(),
                max_frames: max,
            });
        });
    }

    // HIWAVE PATCH (interrupt): stop when the word names the running turn,
    // or is `u64::MAX` (every turn). 0 never stops anything.
    #[inline]
    pub(crate) fn check_interrupt(&self) -> JsResult<()> {
        // HIWAVE PATCH (stop finally): while a stop runs `finally` blocks,
        // the word naming the stopped turn is what they run despite; the
        // grace's end stops them, and `u64::MAX` (Stop) still does.
        let unwinding = self.vm.stop_unwind.as_ref();
        if let Some(stop) = unwinding {
            if std::time::Instant::now() >= stop.until {
                return Err(RuntimeLimitError::Interrupted.into());
            }
            // Left the block by `break` or `continue`: the stop goes on.
            if let Some((depth, finally)) = stop.running.last()
                && *depth == self.vm.frames.len()
                && !(finally.entry..finally.exit_end).contains(&self.vm.frame().pc.saturating_sub(1))
            {
                return Err(stop.error.into());
            }
        }
        // END HIWAVE PATCH
        if let Some(word) = &self.vm.interrupt {
            let value = word.load(std::sync::atomic::Ordering::Relaxed);
            if value != 0
                && (value == u64::MAX || (value == self.vm.interrupt_turn && unwinding.is_none()))
            {
                // HIWAVE PATCH (interrupt diagnostics): one line per stopped
                // turn, and only after the word has already tripped.
                interrupt_diag::note_interrupt_stop(value, self.vm.interrupt_turn);
                return Err(RuntimeLimitError::Interrupted.into());
            }
        }
        Ok(())
    }
    // END HIWAVE PATCH

    // HIWAVE PATCH (stop finally)
    /// Whether `err` is a stop whose unwinding runs `finally` blocks now:
    /// the interrupt word or the loop limit (an embedder's deadline is
    /// reported as one), with a grace configured, not Stop (`u64::MAX`),
    /// and the grace not over.
    fn stop_may_run_finally(&self, err: &JsError) -> Option<RuntimeLimitError> {
        let grace = self.vm.stop_finally_grace?;
        if self.vm.stop_finally_blocked {
            return None;
        }
        let kind = match err.as_engine() {
            Some(crate::error::EngineError::RuntimeLimit(
                kind @ (RuntimeLimitError::Interrupted | RuntimeLimitError::LoopIteration),
            )) => *kind,
            _ => return None,
        };
        if let Some(word) = &self.vm.interrupt
            && word.load(std::sync::atomic::Ordering::Relaxed) == u64::MAX
        {
            return None;
        }
        let _ = grace;
        match &self.vm.stop_unwind {
            Some(stop) if std::time::Instant::now() >= stop.until => None,
            Some(stop) => Some(stop.error),
            None => Some(kind),
        }
    }

    /// The stop `err` leaves the running code. If a `try` with a `finally`
    /// encloses it, in this frame or a caller's up to the frame the Rust
    /// caller entered, the frames above that one are left, and its
    /// `finally` block runs, never a `catch`. `None` when there is none, or
    /// the stop runs no `finally` (`stop_may_run_finally`): the stop then
    /// unwinds to the Rust caller, as upstream.
    fn enter_stop_finally(&mut self, err: &JsError) -> Option<ControlFlow<CompletionRecord>> {
        let kind = self.stop_may_run_finally(err)?;
        // Find the frame and its innermost `finally` first, so frames are
        // left only when there is one.
        let top = self.vm.frames.len() - 1;
        let mut index = top;
        let mut pc = self.vm.frame().pc.saturating_sub(1);
        let found = loop {
            let frame = &self.vm.frames[index];
            let finally = frame
                .code_block()
                .handlers
                .iter()
                .rev()
                .find(|handler| handler.contains(pc) && handler.stop_finally.is_some())
                .copied();
            if let Some(handler) = finally {
                break Some((index, pc, handler));
            }
            if frame.exit_early() || index <= 1 {
                break None;
            }
            index -= 1;
            pc = self.vm.frames[index].pc;
        };
        let Some((index, leaving_pc, handler)) = found else {
            // The stop leaves for the Rust caller from here: the `finally`
            // blocks of the frames it leaves are over.
            let depth = index + 1;
            if let Some(stop) = self.vm.stop_unwind.as_mut() {
                stop.running.retain(|(at, _)| *at < depth);
            }
            return None;
        };
        let now = std::time::Instant::now();
        let grace = self.vm.stop_finally_grace.unwrap_or_default();
        let stop = self.vm.stop_unwind.get_or_insert_with(|| StopUnwind {
            error: kind,
            until: now + grace,
            running: Vec::new(),
        });
        // The `finally` blocks being left: those of the frames above, and
        // the one in this frame the stop is leaving.
        let depth = index + 1;
        stop.running.retain(|(at, finally)| {
            *at < depth
                || (*at == depth && !(finally.entry..finally.exit_end).contains(&leaving_pc))
        });
        let finally = handler.stop_finally.expect("found by it");
        stop.running.push((depth, finally));
        let mut last = None;
        while self.vm.frames.len() > depth {
            last = self.vm.pop_frame();
        }
        if let Some(frame) = last {
            self.vm.stack.truncate_to_frame(&frame);
        }
        let frame = self.vm.frame_mut();
        let environment_sp = frame.env_fp + handler.environment_count;
        frame.pc = finally.entry;
        frame.environments.truncate(environment_sp as usize);
        self.vm.pending_exception = None;
        Some(ControlFlow::Continue(()))
    }

    /// The innermost `finally` the stop runs is in the running frame.
    fn in_stop_finally_frame(&self) -> bool {
        self.vm
            .stop_unwind
            .as_ref()
            .and_then(|stop| stop.running.last())
            .is_some_and(|(depth, _)| *depth == self.vm.frames.len())
    }

    /// The running `finally` the stop entered has ended at `pc`, at its
    /// rethrow: the stop goes on.
    pub(crate) fn stop_finally_ends_at(&self, pc: u32) -> bool {
        self.in_stop_finally_frame()
            && self
                .vm
                .stop_unwind
                .as_ref()
                .and_then(|stop| stop.running.last())
                .is_some_and(|(_, finally)| (finally.exit..finally.exit_end).contains(&pc))
    }

    /// Go on with the stop from here: the next `finally` out, or the Rust
    /// caller.
    pub(crate) fn resume_stop(&mut self) -> ControlFlow<CompletionRecord> {
        let kind = self
            .vm
            .stop_unwind
            .as_ref()
            .map_or(RuntimeLimitError::Interrupted, |stop| stop.error);
        self.vm.pending_exception = None;
        self.handle_error(kind.into())
    }

    /// An exception thrown while a stop runs a `finally` would be caught
    /// outside that block (by a `catch` around the `try` it belongs to, or
    /// in a caller): the stop goes on instead. One caught inside the block,
    /// or in a function it calls, is the page's own.
    pub(crate) fn stop_claims_throw(&self) -> bool {
        let Some((depth, finally)) = self
            .vm
            .stop_unwind
            .as_ref()
            .and_then(|stop| stop.running.last().copied())
        else {
            return false;
        };
        let mut index = self.vm.frames.len() - 1;
        let mut pc = self.vm.frame().pc.saturating_sub(1);
        loop {
            if index + 1 < depth {
                return true;
            }
            let frame = &self.vm.frames[index];
            if let Some((_, handler)) = frame.code_block().find_handler(pc) {
                return index + 1 == depth
                    && !(handler.start.as_u32() >= finally.entry
                        && handler.end.as_u32() <= finally.exit);
            }
            if frame.exit_early() {
                // Leaving for a native caller, which hands it back to the
                // VM: that run decides, unless this is the `finally`'s frame.
                return index + 1 == depth;
            }
            if index == 0 {
                return true;
            }
            index -= 1;
            pc = self.vm.frames[index].pc;
        }
    }
    // END HIWAVE PATCH
}

/// Yields once to the executor.
fn yield_now() -> impl Future<Output = ()> {
    struct YieldNow(bool);

    impl Future for YieldNow {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
            if self.0 {
                task::Poll::Ready(())
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                task::Poll::Pending
            }
        }
    }

    YieldNow(false)
}
