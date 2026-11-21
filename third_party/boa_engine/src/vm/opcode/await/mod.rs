use super::RegisterOperand;
use crate::{
    Context, JsArgs, JsExpect, JsResult, JsValue,
    builtins::{
        Promise, async_generator::AsyncGenerator, generator::GeneratorContext,
        promise::PromiseCapability,
    },
    error::PanicError,
    js_string,
    native_function::NativeFunction,
    object::FunctionObjectBuilder,
    vm::{CompletionRecord, GeneratorResumeKind, opcode::Operation},
};
use boa_gc::Gc;
use std::{cell::Cell, ops::ControlFlow};

// HIWAVE PATCH (interrupt)
use crate::{
    JsObject,
    module::{Module, abandon_interrupted_module_body},
    vm::{ActiveRunnable, CallFrame, CodeBlock},
};

/// HIWAVE PATCH (interrupt): the module and code block of a suspended async
/// body run by a module, to abandon that module if the body is its top level
/// and the interrupt stops it.
fn module_body(r#gen: &GeneratorContext) -> Option<(Module, Gc<CodeBlock>)> {
    let frame = r#gen.call_frame.as_ref()?;
    match &frame.active_runnable {
        Some(ActiveRunnable::Module(module)) => Some((module.clone(), frame.code_block.clone())),
        _ => None,
    }
}

/// HIWAVE PATCH (interrupt): the reject function of a suspended async
/// function's promise, to settle it if the interrupt stops the body. `None`
/// for an async generator (abandoned on its own) and for a module's top
/// level, which has no function object (abandoned with its module).
fn async_function_reject(r#gen: &GeneratorContext) -> Option<JsObject> {
    let frame = r#gen.call_frame.as_ref()?;
    let code = frame.code_block();
    if !code.is_async() || code.is_generator() {
        return None;
    }
    r#gen.stack.stack.get(frame.function_index())?.as_object()?;
    r#gen
        .stack
        .get_register(frame, CallFrame::PROMISE_CAPABILITY_REJECT_REGISTER_INDEX)?
        .as_object()
}

/// `Await` implements the Opcode Operation for `Opcode::Await`
///
/// Operation:
///  - Stops the current Async function and schedules it to resume later.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Await;

impl Await {
    #[inline(always)]
    pub(super) fn operation(
        value: RegisterOperand,
        context: &mut Context,
    ) -> ControlFlow<CompletionRecord> {
        let value = context.vm.get_register(value.into());

        // 2. Let promise be ? PromiseResolve(%Promise%, value).
        let promise = match Promise::promise_resolve(
            &context.intrinsics().constructors().promise().constructor(),
            value.clone(),
            context,
        ) {
            Ok(promise) => match promise.downcast::<Promise>().ok() {
                Some(v) => v,
                None => {
                    return context.handle_error(
                        PanicError::new("%Promise% constructor must return a `Promise` object")
                            .into(),
                    );
                }
            },
            Err(err) => return context.handle_error(err),
        };

        let return_value = context
            .vm
            .get_promise_capability()
            .ok()
            .map(|cap| JsValue::from(cap.promise))
            .unwrap_or_default();

        let r#gen = GeneratorContext::from_current(context, None);

        let captures = Gc::new(Cell::new(Some(r#gen)));

        // 3. Let fulfilledClosure be a new Abstract Closure with parameters (value) that captures asyncContext and performs the following steps when called:
        // 4. Let onFulfilled be CreateBuiltinFunction(fulfilledClosure, 1, "", « »).
        let on_fulfilled = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_copy_closure_with_captures(
                |_this, args, captures, context| {
                    // a. Let prevContext be the running execution context.
                    // b. Suspend prevContext.
                    // c. Push asyncContext onto the execution context stack; asyncContext is now the running execution context.
                    // d. Resume the suspended evaluation of asyncContext using NormalCompletion(value) as the result of the operation that suspended it.
                    let mut r#gen = captures.take().js_expect("should only run once")?;

                    // NOTE: We need to get the object before resuming, since it could clear the stack.
                    let async_generator = r#gen.async_generator_object()?;

                    // HIWAVE PATCH (interrupt): the module whose top level this
                    // is, if any (see below).
                    let runnable = module_body(&r#gen);
                    let async_function = async_function_reject(&r#gen);

                    // HIWAVE PATCH (interrupt): keep the completion (was dropped).
                    let completion = r#gen.resume(
                        Some(args.get_or_undefined(0).clone()),
                        GeneratorResumeKind::Normal,
                        context,
                    );

                    if let Some(async_generator) = &async_generator {
                        async_generator
                            .downcast_mut::<AsyncGenerator>()
                            .js_expect("must be async generator")?
                            .context = Some(r#gen);
                    }

                    // HIWAVE PATCH (interrupt): the body takes every catchable
                    // error itself; a runtime limit (the interrupt) unwinds
                    // past it and ends this job instead of being dropped.
                    // A module's top level stopped this way is abandoned, so
                    // later imports of it do not hang; so is an async
                    // generator, so its later calls settle, and an async
                    // function's promise is rejected with an ordinary error.
                    if let CompletionRecord::Throw(err) = completion
                        && !err.is_catchable()
                    {
                        // HIWAVE PATCH (interrupt diagnostics): resumed by the
                        // fulfilled reaction with an uncatchable throw.
                        crate::vm::interrupt_diag::note_interrupt_err(
                            "await.resume.fulfill",
                            resume_interrupt_message(
                                async_function.is_some(),
                                async_generator.is_some(),
                                runnable.is_some(),
                            ),
                            &err,
                        );
                        if let Some((module, code)) = runnable {
                            abandon_interrupted_module_body(&module, &code);
                        }
                        if let Some(async_generator) = &async_generator {
                            AsyncGenerator::abandon_interrupted(async_generator, context);
                        }
                        if let Some(reject) = &async_function {
                            Promise::reject_interrupted(
                                reject,
                                "async function was interrupted",
                                context,
                            );
                        }
                        return Err(err);
                    }
                    // END HIWAVE PATCH

                    // e. Assert: When we reach this step, asyncContext has already been removed from the execution context stack and prevContext is the currently running execution context.
                    // f. Return undefined.
                    Ok(JsValue::undefined())
                },
                captures.clone(),
            ),
        )
        .name(js_string!())
        .length(1)
        .build();

        // 5. Let rejectedClosure be a new Abstract Closure with parameters (reason) that captures asyncContext and performs the following steps when called:
        // 6. Let onRejected be CreateBuiltinFunction(rejectedClosure, 1, "", « »).
        let on_rejected = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_copy_closure_with_captures(
                |_this, args, captures, context| {
                    // a. Let prevContext be the running execution context.
                    // b. Suspend prevContext.
                    // c. Push asyncContext onto the execution context stack; asyncContext is now the running execution context.
                    // d. Resume the suspended evaluation of asyncContext using ThrowCompletion(reason) as the result of the operation that suspended it.
                    // e. Assert: When we reach this step, asyncContext has already been removed from the execution context stack and prevContext is the currently running execution context.
                    // f. Return undefined.
                    let mut r#gen = captures.take().js_expect("should only run once")?;

                    // NOTE: We need to get the object before resuming, since it could clear the stack.
                    let async_generator = r#gen.async_generator_object()?;

                    // HIWAVE PATCH (interrupt): the module whose top level this
                    // is, if any (see below).
                    let runnable = module_body(&r#gen);
                    let async_function = async_function_reject(&r#gen);

                    // HIWAVE PATCH (interrupt): keep the completion (was dropped).
                    let completion = r#gen.resume(
                        Some(args.get_or_undefined(0).clone()),
                        GeneratorResumeKind::Throw,
                        context,
                    );

                    if let Some(async_generator) = &async_generator {
                        async_generator
                            .downcast_mut::<AsyncGenerator>()
                            .js_expect("must be async generator")?
                            .context = Some(r#gen);
                    }

                    // HIWAVE PATCH (interrupt): the body takes every catchable
                    // error itself; a runtime limit (the interrupt) unwinds
                    // past it and ends this job instead of being dropped.
                    // A module's top level stopped this way is abandoned, so
                    // later imports of it do not hang; so is an async
                    // generator, so its later calls settle, and an async
                    // function's promise is rejected with an ordinary error.
                    if let CompletionRecord::Throw(err) = completion
                        && !err.is_catchable()
                    {
                        // HIWAVE PATCH (interrupt diagnostics): resumed by the
                        // rejected reaction with an uncatchable throw.
                        crate::vm::interrupt_diag::note_interrupt_err(
                            "await.resume.reject",
                            resume_interrupt_message(
                                async_function.is_some(),
                                async_generator.is_some(),
                                runnable.is_some(),
                            ),
                            &err,
                        );
                        if let Some((module, code)) = runnable {
                            abandon_interrupted_module_body(&module, &code);
                        }
                        if let Some(async_generator) = &async_generator {
                            AsyncGenerator::abandon_interrupted(async_generator, context);
                        }
                        if let Some(reject) = &async_function {
                            Promise::reject_interrupted(
                                reject,
                                "async function was interrupted",
                                context,
                            );
                        }
                        return Err(err);
                    }
                    // END HIWAVE PATCH

                    Ok(JsValue::undefined())
                },
                captures,
            ),
        )
        .name(js_string!())
        .length(1)
        .build();

        // 7. Perform PerformPromiseThen(promise, onFulfilled, onRejected).
        Promise::perform_promise_then(
            &promise,
            Some(on_fulfilled),
            Some(on_rejected),
            None,
            context,
        );

        context.vm.set_return_value(return_value);
        context.handle_yield()
    }
}

// HIWAVE PATCH (interrupt diagnostics): the message the resume path formats.
// An async function's promise is rejected with the first string; a generator
// or a module body takes the others. The site name still says which reaction
// resumed the body.
fn resume_interrupt_message(
    async_function: bool,
    async_generator: bool,
    module_body: bool,
) -> &'static str {
    if async_function {
        "async function was interrupted"
    } else if async_generator {
        "async generator was interrupted"
    } else if module_body {
        "module evaluation was interrupted"
    } else {
        "the script was interrupted"
    }
}

impl Operation for Await {
    const NAME: &'static str = "Await";
    const INSTRUCTION: &'static str = "INST - Await";
    const COST: u8 = 5;
}

/// `CreatePromiseCapability` implements the Opcode Operation for `Opcode::CreatePromiseCapability`
///
/// Operation:
///  - Create a promise capacity for an async function.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CreatePromiseCapability;

impl CreatePromiseCapability {
    #[inline(always)]
    pub(super) fn operation((): (), context: &mut Context) -> JsResult<()> {
        let promise_capability = PromiseCapability::new(
            &context.intrinsics().constructors().promise().constructor(),
            context,
        )
        .js_expect("cannot fail per spec")?;

        context.vm.set_promise_capability(promise_capability)
    }
}

impl Operation for CreatePromiseCapability {
    const NAME: &'static str = "CreatePromiseCapability";
    const INSTRUCTION: &'static str = "INST - CreatePromiseCapability";
    const COST: u8 = 8;
}
