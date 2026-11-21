//! Boa's implementation of ECMAScript's global `AsyncGenerator` object.
//!
//! More information:
//!  - [ECMAScript reference][spec]
//!
//! [spec]: https://tc39.es/ecma262/#sec-asyncgenerator-objects

use crate::{
    Context, JsArgs, JsData, JsError, JsExpect, JsResult, JsString,
    builtins::{
        Promise,
        generator::GeneratorContext,
        iterable::create_iter_result_object,
        promise::{PromiseCapability, if_abrupt_reject_promise},
    },
    context::intrinsics::Intrinsics,
    error::JsNativeError,
    js_string,
    native_function::NativeFunction,
    object::{CONSTRUCTOR, FunctionObjectBuilder, JsObject},
    property::Attribute,
    realm::Realm,
    string::StaticJsStrings,
    symbol::JsSymbol,
    value::JsValue,
    vm::{CompletionRecord, GeneratorResumeKind},
};
use boa_gc::{Finalize, Trace};
use std::{collections::VecDeque, slice};

use super::{BuiltInBuilder, IntrinsicObject};

/// Indicates the state of an async generator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AsyncGeneratorState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    DrainingQueue,
    Completed,
}

/// `AsyncGeneratorRequest Records`
///
/// More information:
///  - [ECMAScript reference][spec]
///
/// [spec]: https://tc39.es/ecma262/#sec-asyncgeneratorrequest-records
#[derive(Debug, Clone, Finalize, Trace)]
pub(crate) struct AsyncGeneratorRequest {
    /// The `[[Completion]]` slot.
    pub(crate) completion: CompletionRecord,

    /// The `[[Capability]]` slot.
    capability: PromiseCapability,
}

/// The internal representation of an `AsyncGenerator` object.
#[derive(Debug, Finalize, Trace, JsData)]
pub struct AsyncGenerator {
    /// The `[[AsyncGeneratorState]]` internal slot.
    #[unsafe_ignore_trace]
    pub(crate) state: AsyncGeneratorState,

    /// The `[[AsyncGeneratorContext]]` internal slot.
    pub(crate) context: Option<GeneratorContext>,

    /// The `[[AsyncGeneratorQueue]]` internal slot.
    pub(crate) queue: VecDeque<AsyncGeneratorRequest>,
}

impl IntrinsicObject for AsyncGenerator {
    fn init(realm: &Realm) {
        BuiltInBuilder::with_intrinsic::<Self>(realm)
            .prototype(
                realm
                    .intrinsics()
                    .objects()
                    .iterator_prototypes()
                    .async_iterator(),
            )
            .static_method(Self::next, js_string!("next"), 1)
            .static_method(Self::r#return, js_string!("return"), 1)
            .static_method(Self::throw, js_string!("throw"), 1)
            .static_property(
                JsSymbol::to_string_tag(),
                Self::NAME,
                Attribute::CONFIGURABLE,
            )
            .static_property(
                CONSTRUCTOR,
                realm
                    .intrinsics()
                    .constructors()
                    .async_generator_function()
                    .prototype(),
                Attribute::CONFIGURABLE,
            )
            .build();
    }

    fn get(intrinsics: &Intrinsics) -> JsObject {
        intrinsics.objects().async_generator()
    }
}

impl AsyncGenerator {
    const NAME: JsString = StaticJsStrings::ASYNC_GENERATOR;

    /// `AsyncGenerator.prototype.next ( value )`
    ///
    /// More information:
    ///  - [ECMAScript reference][spec]
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-asyncgenerator-prototype-next
    pub(crate) fn next(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        // 1. Let generator be the this value.
        let generator = this;

        // 2. Let promiseCapability be ! NewPromiseCapability(%Promise%).
        let promise_capability = PromiseCapability::new(
            &context.intrinsics().constructors().promise().constructor(),
            context,
        )
        .js_expect("cannot fail with promise constructor")?;

        // 3. Let result be Completion(AsyncGeneratorValidate(generator, empty)).
        // 4. IfAbruptRejectPromise(result, promiseCapability).
        let result: JsResult<_> = generator.as_object().ok_or_else(|| {
            JsNativeError::typ()
                .with_message("generator resumed on non generator object")
                .into()
        });
        let generator = if_abrupt_reject_promise!(result, promise_capability, context);
        let result: JsResult<_> = generator.clone().downcast::<Self>().map_err(|_| {
            JsNativeError::typ()
                .with_message("generator resumed on non generator object")
                .into()
        });
        let generator = if_abrupt_reject_promise!(result, promise_capability, context);

        Self::inner_next(
            &generator,
            promise_capability,
            args.get_or_undefined(0).clone(),
            context,
        )
        .map(JsValue::from)
    }

    pub(crate) fn inner_next(
        generator: &JsObject<AsyncGenerator>,
        cap: PromiseCapability,
        value: JsValue,
        context: &mut Context,
    ) -> JsResult<JsObject> {
        // 5. Let state be generator.[[AsyncGeneratorState]].
        let state = generator.borrow().data().state;

        // 6. If state is completed, then
        if state == AsyncGeneratorState::Completed {
            // a. Let iteratorResult be CreateIterResultObject(undefined, true).
            let iterator_result = create_iter_result_object(JsValue::undefined(), true, context);

            // b. Perform ! Call(promiseCapability.[[Resolve]], undefined, « iteratorResult »).
            cap.resolve()
                .call(&JsValue::undefined(), &[iterator_result], context)?;

            // c. Return promiseCapability.[[Promise]].
            return Ok(cap.promise);
        }

        // 7. Let completion be NormalCompletion(value).
        let completion = CompletionRecord::Normal(value);

        // 8. Perform AsyncGeneratorEnqueue(generator, completion, promiseCapability).
        Self::enqueue(generator, completion.clone(), cap.clone());

        // 9. If state is either suspendedStart or suspendedYield, then
        if state == AsyncGeneratorState::SuspendedStart
            || state == AsyncGeneratorState::SuspendedYield
        {
            // a. Perform AsyncGeneratorResume(generator, completion).
            Self::resume(generator, completion, context)?;
        }

        // 11. Return promiseCapability.[[Promise]].
        Ok(cap.promise)
    }

    /// `AsyncGenerator.prototype.return ( value )`
    ///
    /// More information:
    ///  - [ECMAScript reference][spec]
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-asyncgenerator-prototype-return
    pub(crate) fn r#return(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        // 1. Let generator be the this value.
        let generator = this;

        // 2. Let promiseCapability be ! NewPromiseCapability(%Promise%).
        let promise_capability = PromiseCapability::new(
            &context.intrinsics().constructors().promise().constructor(),
            context,
        )
        .js_expect("cannot fail with promise constructor")?;

        // 3. Let result be Completion(AsyncGeneratorValidate(generator, empty)).
        // 4. IfAbruptRejectPromise(result, promiseCapability).
        let result: JsResult<_> = generator.as_object().ok_or_else(|| {
            JsNativeError::typ()
                .with_message("generator resumed on non generator object")
                .into()
        });
        let generator_object = if_abrupt_reject_promise!(result, promise_capability, context);
        let result: JsResult<_> = generator_object.clone().downcast::<Self>().map_err(|_| {
            JsNativeError::typ()
                .with_message("generator resumed on non generator object")
                .into()
        });
        let generator = if_abrupt_reject_promise!(result, promise_capability, context);

        Self::inner_return(
            &generator,
            promise_capability,
            args.get_or_undefined(0).clone(),
            context,
        )
        .map(JsValue::from)
    }

    pub(crate) fn inner_return(
        generator: &JsObject<AsyncGenerator>,
        cap: PromiseCapability,
        return_value: JsValue,
        context: &mut Context,
    ) -> JsResult<JsObject> {
        // 5. Let completion be Completion Record { [[Type]]: return, [[Value]]: value, [[Target]]: empty }.
        let completion = CompletionRecord::Return(return_value.clone());

        // 6. Perform AsyncGeneratorEnqueue(generator, completion, promiseCapability).
        Self::enqueue(generator, completion.clone(), cap.clone());

        // 7. Let state be generator.[[AsyncGeneratorState]].
        let state = generator.borrow().data().state;

        // 8. If state is either suspended-start or completed, then
        if state == AsyncGeneratorState::SuspendedStart || state == AsyncGeneratorState::Completed {
            // a. Set generator.[[AsyncGeneratorState]] to draining-queue.
            generator.borrow_mut().data_mut().state = AsyncGeneratorState::DrainingQueue;

            // b. Perform ! AsyncGeneratorAwaitReturn(generator).
            Self::await_return(generator, return_value, context)?;
        }
        // 9. Else if state is suspended-yield, then
        else if state == AsyncGeneratorState::SuspendedYield {
            // a. Perform AsyncGeneratorResume(generator, completion).
            Self::resume(generator, completion, context)?;
        }
        // 10. Else,
        //     a. Assert: state is either executing or draining-queue.

        // 11. Return promiseCapability.[[Promise]].
        Ok(cap.promise)
    }

    /// `AsyncGenerator.prototype.throw ( exception )`
    ///
    /// More information:
    ///  - [ECMAScript reference][spec]
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-asyncgenerator-prototype-throw
    pub(crate) fn throw(
        this: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<JsValue> {
        // 1. Let generator be the this value.
        let generator = this;

        // 2. Let promiseCapability be ! NewPromiseCapability(%Promise%).
        let promise_capability = PromiseCapability::new(
            &context.intrinsics().constructors().promise().constructor(),
            context,
        )
        .js_expect("cannot fail with promise constructor")?;

        // 3. Let result be Completion(AsyncGeneratorValidate(generator, empty)).
        // 4. IfAbruptRejectPromise(result, promiseCapability).
        let result: JsResult<_> = generator.as_object().ok_or_else(|| {
            JsNativeError::typ()
                .with_message("generator resumed on non generator object")
                .into()
        });
        let generator_object = if_abrupt_reject_promise!(result, promise_capability, context);
        let result: JsResult<_> = generator_object.clone().downcast::<Self>().map_err(|_| {
            JsNativeError::typ()
                .with_message("generator resumed on non generator object")
                .into()
        });
        let generator = if_abrupt_reject_promise!(result, promise_capability, context);
        Self::inner_throw(
            &generator,
            promise_capability,
            args.get_or_undefined(0).clone(),
            context,
        )
        .map(JsValue::from)
    }

    pub(crate) fn inner_throw(
        generator: &JsObject<AsyncGenerator>,
        cap: PromiseCapability,
        error_value: JsValue,
        context: &mut Context,
    ) -> JsResult<JsObject> {
        let mut r#gen = generator.borrow_mut();

        // 5. Let state be generator.[[AsyncGeneratorState]].
        let mut state = r#gen.data().state;

        // 6. If state is suspendedStart, then
        if state == AsyncGeneratorState::SuspendedStart {
            // a. Set generator.[[AsyncGeneratorState]] to completed.
            r#gen.data_mut().state = AsyncGeneratorState::Completed;
            r#gen.data_mut().context = None;

            // b. Set state to completed.
            state = AsyncGeneratorState::Completed;
        }

        drop(r#gen);

        // 7. If state is completed, then
        if state == AsyncGeneratorState::Completed {
            // a. Perform ! Call(promiseCapability.[[Reject]], undefined, « exception »).
            cap.reject().call(
                &JsValue::undefined(),
                slice::from_ref(&error_value),
                context,
            )?;

            // b. Return promiseCapability.[[Promise]].
            return Ok(cap.promise);
        }

        // 8. Let completion be ThrowCompletion(exception).
        let completion = CompletionRecord::Throw(JsError::from_opaque(error_value));

        // 9. Perform AsyncGeneratorEnqueue(generator, completion, promiseCapability).
        Self::enqueue(generator, completion.clone(), cap.clone());

        // 10. If state is suspended-yield, then
        if state == AsyncGeneratorState::SuspendedYield {
            // a. Perform AsyncGeneratorResume(generator, completion).
            Self::resume(generator, completion, context)?;
        }

        // 11. Else,
        //     a. Assert: state is either executing or draining-queue.

        // 12. Return promiseCapability.[[Promise]].
        Ok(cap.promise)
    }

    /// `AsyncGeneratorEnqueue ( generator, completion, promiseCapability )`
    ///
    /// More information:
    ///  - [ECMAScript reference][spec]
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-asyncgeneratorenqueue
    pub(crate) fn enqueue(
        generator: &JsObject<AsyncGenerator>,
        completion: CompletionRecord,
        promise_capability: PromiseCapability,
    ) {
        let mut r#gen = generator.borrow_mut();
        // 1. Let request be AsyncGeneratorRequest { [[Completion]]: completion, [[Capability]]: promiseCapability }.
        let request = AsyncGeneratorRequest {
            completion,
            capability: promise_capability,
        };

        // 2. Append request to the end of generator.[[AsyncGeneratorQueue]].
        r#gen.data_mut().queue.push_back(request);
    }

    /// `AsyncGeneratorCompleteStep ( generator, completion, done [ , realm ] )`
    ///
    /// More information:
    ///  - [ECMAScript reference][spec]
    ///
    /// # Errors
    ///
    /// Returns `EngineError::Panic` if the async generator request queue of `generator` is empty.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-asyncgeneratorcompletestep
    pub(crate) fn complete_step(
        generator: &JsObject<AsyncGenerator>,
        completion: JsResult<JsValue>,
        done: bool,
        realm: Option<Realm>,
        context: &mut Context,
    ) -> JsResult<()> {
        // 1. Assert: generator.[[AsyncGeneratorQueue]] is not empty.
        // 2. Let next be the first element of generator.[[AsyncGeneratorQueue]].
        // 3. Remove the first element from generator.[[AsyncGeneratorQueue]].
        let next = generator
            .borrow_mut()
            .data_mut()
            .queue
            .pop_front()
            .js_expect("1. Assert: generator.[[AsyncGeneratorQueue]] is not empty.")?;

        // 4. Let promiseCapability be next.[[Capability]].
        let promise_capability = &next.capability;

        // 5. Let value be completion.[[Value]].
        match completion {
            // 6. If completion is a throw completion, then
            Err(e) => {
                // HIWAVE PATCH (interrupt): a runtime limit (the interrupt)
                // cannot be handed to script; the request is rejected with
                // an ordinary error instead and the stop goes on.
                let reason = match e.into_opaque(context) {
                    Ok(reason) => reason,
                    Err(stop) => {
                        crate::vm::interrupt_diag::note_interrupt(
                            "async_generator.complete",
                            INTERRUPTED,
                        );
                        Promise::reject_interrupted(
                            promise_capability.reject(),
                            INTERRUPTED,
                            context,
                        );
                        return Err(stop);
                    }
                };
                // END HIWAVE PATCH
                // a. Perform ! Call(promiseCapability.[[Reject]], undefined, « value »).
                let result =
                    promise_capability
                        .reject()
                        .call(&JsValue::undefined(), &[reason], context);
                // HIWAVE PATCH (interrupt): refused at its entry, the request
                // would stay pending; it is rejected exempt instead.
                settle_refused(&result, promise_capability, context);
                result?;
                // END HIWAVE PATCH
            }

            // 7. Else,
            Ok(value) => {
                // a. Assert: completion is a normal completion.
                // b. If realm is present, then
                let iterator_result = if let Some(realm) = realm {
                    // i. Let oldRealm be the running execution context's Realm.
                    // ii. Set the running execution context's Realm to realm.
                    let old_realm = context.enter_realm(realm);

                    // iii. Let iteratorResult be CreateIteratorResultObject(value, done).
                    let iterator_result = create_iter_result_object(value, done, context);

                    // iv. Set the running execution context's Realm to oldRealm.
                    context.enter_realm(old_realm);

                    iterator_result
                } else {
                    // c. Else,
                    //     i. Let iteratorResult be CreateIteratorResultObject(value, done).
                    create_iter_result_object(value, done, context)
                };

                // d. Perform ! Call(promiseCapability.[[Resolve]], undefined, « iteratorResult »).
                let result = promise_capability.resolve().call(
                    &JsValue::undefined(),
                    &[iterator_result],
                    context,
                );
                // HIWAVE PATCH (interrupt): as for the reject above.
                settle_refused(&result, promise_capability, context);
                result?;
                // END HIWAVE PATCH
            }
        }
        // 8. Return unused.
        Ok(())
    }

    /// `AsyncGeneratorResume ( generator, completion )`
    ///
    /// More information:
    ///  - [ECMAScript reference][spec]
    ///
    /// # Errors
    ///
    /// Returns `EngineError::Panic` if `generator` is neither in the `SuspendedStart` nor in the `SuspendedYield` states.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-asyncgeneratorresume
    pub(crate) fn resume(
        generator: &JsObject<AsyncGenerator>,
        completion: CompletionRecord,
        context: &mut Context,
    ) -> JsResult<()> {
        // 1. Assert: generator.[[AsyncGeneratorState]] is either suspended-start or suspended-yield.
        assert!(matches!(
            generator.borrow().data().state,
            AsyncGeneratorState::SuspendedStart | AsyncGeneratorState::SuspendedYield
        ));

        // 2. Let genContext be generator.[[AsyncGeneratorContext]].
        let mut generator_context = generator
            .borrow_mut()
            .data_mut()
            .context
            .take()
            .js_expect("generator context cannot be empty here")?;

        // 5. Set generator.[[AsyncGeneratorState]] to executing.
        generator.borrow_mut().data_mut().state = AsyncGeneratorState::Executing;

        let (value, resume_kind) = match completion {
            CompletionRecord::Normal(val) => (val, GeneratorResumeKind::Normal),
            CompletionRecord::Return(val) => (val, GeneratorResumeKind::Return),
            CompletionRecord::Throw(err) => (err.into_opaque(context)?, GeneratorResumeKind::Throw),
        };

        // 3. Let callerContext be the running execution context.
        // 4. Suspend callerContext.
        // 6. Push genContext onto the execution context stack; genContext is now the running execution context.
        let result = generator_context.resume(Some(value), resume_kind, context);

        // 7. Resume the suspended evaluation of genContext using completion as the result of the operation that suspended it. Let result be the Completion Record returned by the resumed computation.
        generator.borrow_mut().data_mut().context = Some(generator_context);

        // 8. Assert: result is never an abrupt completion.
        // HIWAVE PATCH (interrupt): except a runtime limit (the interrupt),
        // which no handler in the body takes; it ends the job, and the
        // generator is abandoned so later calls settle.
        if let CompletionRecord::Throw(err) = result {
            crate::vm::interrupt_diag::note_interrupt("async_generator.resume", INTERRUPTED);
            Self::abandon_interrupted(&generator.clone().upcast(), context);
            return Err(err);
        }
        // END HIWAVE PATCH

        // 9. Assert: When we return here, genContext has already been removed from the execution context stack and
        //    callerContext is the currently running execution context.
        // 10. Return unused.
        Ok(())
    }

    // HIWAVE PATCH (interrupt)
    /// The interrupt stopped this generator's body while it ran. Left
    /// `executing`, it would queue every later `next`/`return`/`throw`
    /// forever. It becomes `completed` (so later calls settle as on a
    /// finished generator), and every request already queued, the stopped
    /// one included, is rejected with an ordinary, catchable error. The
    /// reject functions only enqueue jobs and run no script, so they are
    /// called exempt from the interrupt check, which lets this run in the
    /// stopped turn.
    pub(crate) fn abandon_interrupted(generator: &JsObject, context: &mut Context) {
        let queue = {
            let Some(mut r#gen) = generator.downcast_mut::<Self>() else {
                return;
            };
            r#gen.state = AsyncGeneratorState::Completed;
            r#gen.context = None;
            std::mem::take(&mut r#gen.queue)
        };
        crate::vm::interrupt_diag::note_interrupt("async_generator.abandon", INTERRUPTED);
        for request in queue {
            Promise::reject_interrupted(request.capability.reject(), INTERRUPTED, context);
        }
    }

    /// One step of `AsyncGeneratorAwaitReturn`'s closures (c. and d.), run
    /// only if the interrupt has not stopped the turn. If it has, or stops a
    /// step, the generator is abandoned: left `draining-queue`, every later
    /// call would queue forever.
    fn await_return_step(
        generator: &JsObject<AsyncGenerator>,
        result: JsResult<JsValue>,
        context: &mut Context,
    ) -> JsResult<JsValue> {
        // Abandoned while the returned value was awaited: nothing is left.
        if generator.borrow().data().state == AsyncGeneratorState::Completed {
            return Ok(JsValue::undefined());
        }
        let step = context
            .check_interrupt()
            .and_then(|()| Self::complete_step(generator, result, true, None, context))
            .and_then(|()| Self::drain_queue(generator, context));
        if let Err(err) = &step
            && !err.is_catchable()
        {
            Self::abandon_interrupted(&generator.clone().upcast(), context);
        }
        step.map(|()| JsValue::undefined())
    }
    // END HIWAVE PATCH

    /// `AsyncGeneratorAwaitReturn ( generator )`
    ///
    /// More information:
    ///  - [ECMAScript reference][spec]
    ///
    /// # Errors
    ///
    /// Returns `EngineError::Panic` if `generator` is not in the `DrainingQueue` state.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-asyncgeneratorawaitreturn
    pub(crate) fn await_return(
        generator: &JsObject<AsyncGenerator>,
        value: JsValue,
        context: &mut Context,
    ) -> JsResult<()> {
        // 1. Assert: generator.[[AsyncGeneratorState]] is draining-queue.
        assert_eq!(
            generator.borrow().data().state,
            AsyncGeneratorState::DrainingQueue
        );

        // 2. Let queue be generator.[[AsyncGeneratorQueue]].
        // 3. Assert: queue is not empty.
        // 4. Let next be the first element of queue.
        // 5. Let completion be Completion(next.[[Completion]]).
        // 6. Assert: completion is a return completion.

        // 7. Let promiseCompletion be Completion(PromiseResolve(%Promise%, completion.[[Value]])).
        let promise_completion = Promise::promise_resolve(
            &context.intrinsics().constructors().promise().constructor(),
            value,
            context,
        );

        let promise = match promise_completion {
            Ok(value) => value
                .downcast::<Promise>()
                .ok()
                .js_expect("%Promise% constructor must always return a Promise object")?,
            // 8. If promiseCompletion is an abrupt completion, then
            Err(e) => {
                // HIWAVE PATCH (interrupt): stopped in `PromiseResolve` (a
                // `constructor` getter), the generator is abandoned rather
                // than left `draining-queue`.
                if !e.is_catchable() {
                    Self::abandon_interrupted(&generator.clone().upcast(), context);
                    return Err(e);
                }
                // END HIWAVE PATCH
                // a. Perform AsyncGeneratorCompleteStep(generator, promiseCompletion, true).
                // HIWAVE PATCH (interrupt): a catchable `PromiseResolve` error
                // whose rejection the stop refused abandons the generator
                // too, as in `await_return_step`.
                let step = Self::complete_step(generator, Err(e), true, None, context);
                if let Err(err) = &step
                    && !err.is_catchable()
                {
                    Self::abandon_interrupted(&generator.clone().upcast(), context);
                }
                step?;
                // END HIWAVE PATCH
                // b. Perform AsyncGeneratorDrainQueue(generator).
                Self::drain_queue(generator, context)?;
                // c. Return unused.
                return Ok(());
            }
        };

        // 9. Assert: promiseCompletion is a normal completion.
        // 10. Let promise be promiseCompletion.[[Value]].
        // 11. Let fulfilledClosure be a new Abstract Closure with parameters (value) that captures generator and performs the following steps when called:
        // 12. Let onFulfilled be CreateBuiltinFunction(fulfilledClosure, 1, "", « »).
        let on_fulfilled = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_copy_closure_with_captures(
                |_this, args, generator, context| {
                    // HIWAVE PATCH (interrupt): not run in a stopped turn (see
                    // `new_promise_reaction_job`, which lets engine reactions
                    // in); the generator is abandoned instead. The assert of
                    // step a. holds unless it was abandoned already.
                    // a. Assert: generator.[[AsyncGeneratorState]] is draining-queue.

                    // b. Let result be NormalCompletion(value).
                    let result = Ok(args.get_or_undefined(0).clone());

                    // c. Perform AsyncGeneratorCompleteStep(generator, result, true).
                    // d. Perform AsyncGeneratorDrainQueue(generator).
                    // e. Return undefined.
                    Self::await_return_step(generator, result, context)
                    // END HIWAVE PATCH
                },
                generator.clone(),
            ),
        )
        .name(js_string!())
        .length(1)
        .build();

        // 13. Let rejectedClosure be a new Abstract Closure with parameters (reason) that captures generator and performs the following steps when called:
        // 14. Let onRejected be CreateBuiltinFunction(rejectedClosure, 1, "", « »).
        let on_rejected = FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_copy_closure_with_captures(
                |_this, args, generator, context| {
                    // HIWAVE PATCH (interrupt): as in the fulfilled closure.
                    // a. Assert: generator.[[AsyncGeneratorState]] is draining-queue.

                    // b. Let result be ThrowCompletion(reason).
                    let result = Err(JsError::from_opaque(args.get_or_undefined(0).clone()));

                    // c. Perform AsyncGeneratorCompleteStep(generator, result, true).
                    // d. Perform AsyncGeneratorDrainQueue(generator).
                    // e. Return undefined.
                    Self::await_return_step(generator, result, context)
                    // END HIWAVE PATCH
                },
                generator.clone(),
            ),
        )
        .name(js_string!())
        .length(1)
        .build();

        // 15. Perform PerformPromiseThen(promise, onFulfilled, onRejected).
        Promise::perform_promise_then(
            &promise,
            Some(on_fulfilled),
            Some(on_rejected),
            None,
            context,
        );

        // 16. Return unused.
        Ok(())
    }

    /// `AsyncGeneratorDrainQueue ( generator )`
    ///
    /// More information:
    ///  - [ECMAScript reference][spec]
    ///
    /// # Errors
    ///
    /// Returns `EngineError::Panic` if `generator` is not in the `DrainingQueue` state.
    ///
    /// [spec]: https://tc39.es/ecma262/#sec-asyncgeneratordrainqueue
    pub(crate) fn drain_queue(
        generator: &JsObject<AsyncGenerator>,
        context: &mut Context,
    ) -> JsResult<()> {
        // HIWAVE PATCH (interrupt): a drain the interrupt stops abandons the
        // generator; left `draining-queue`, every later call would queue
        // forever.
        let result = Self::drain_queue_steps(generator, context);
        if let Err(err) = &result
            && !err.is_catchable()
        {
            Self::abandon_interrupted(&generator.clone().upcast(), context);
        }
        result
    }

    // HIWAVE PATCH (interrupt): upstream's `drain_queue`, renamed.
    fn drain_queue_steps(
        generator: &JsObject<AsyncGenerator>,
        context: &mut Context,
    ) -> JsResult<()> {
        // END HIWAVE PATCH
        // 1. Assert: generator.[[AsyncGeneratorState]] is draining-queue.
        assert_eq!(
            generator.borrow().data().state,
            AsyncGeneratorState::DrainingQueue
        );

        // 2. Let queue be generator.[[AsyncGeneratorQueue]].
        // 3. If queue is empty, then
        if generator.borrow().data().queue.is_empty() {
            // a. Set generator.[[AsyncGeneratorState]] to completed.
            generator.borrow_mut().data_mut().state = AsyncGeneratorState::Completed;
            generator.borrow_mut().data_mut().context = None;
            // b. Return unused.
            return Ok(());
        }

        // 4. Let done be false.
        // 5. Repeat, while done is false,
        loop {
            // a. Let next be the first element of queue.
            let next = generator
                .borrow()
                .data()
                .queue
                .front()
                .js_expect("must have entry")?
                .completion
                .clone();

            // b. Let completion be Completion(next.[[Completion]]).
            match next {
                // c. If completion is a return completion, then
                CompletionRecord::Return(val) => {
                    // i. Perform AsyncGeneratorAwaitReturn(generator).
                    Self::await_return(generator, val, context)?;

                    // ii. Set done to true.
                    break;
                }
                // d. Else,
                completion => {
                    // i. If completion is a normal completion, then
                    //     1. Set completion to NormalCompletion(undefined).
                    let completion = completion.consume().map(|_| JsValue::undefined());

                    // ii. Perform AsyncGeneratorCompleteStep(generator, completion, true).
                    Self::complete_step(generator, completion, true, None, context)?;

                    // iii. If queue is empty, then
                    if generator.borrow().data().queue.is_empty() {
                        // 1. Set generator.[[AsyncGeneratorState]] to completed.
                        generator.borrow_mut().data_mut().state = AsyncGeneratorState::Completed;
                        generator.borrow_mut().data_mut().context = None;
                        // 2. Set done to true.
                        break;
                    }
                }
            }
        }

        // 6. Return unused.
        Ok(())
    }
}

// HIWAVE PATCH (interrupt)
/// The error a request is rejected with when the interrupt stopped the
/// generator before it could answer it.
const INTERRUPTED: &str = "async generator was interrupted";

/// A request's `resolve`/`reject` that the interrupt refused at its entry
/// left it pending; it is rejected with an ordinary error instead (a no-op
/// if the call got far enough to settle it).
fn settle_refused(
    result: &JsResult<JsValue>,
    capability: &PromiseCapability,
    context: &mut Context,
) {
    if let Err(err) = result
        && !err.is_catchable()
    {
        crate::vm::interrupt_diag::note_interrupt("async_generator.settle_refused", INTERRUPTED);
        Promise::reject_interrupted(capability.reject(), INTERRUPTED, context);
    }
}
// END HIWAVE PATCH
