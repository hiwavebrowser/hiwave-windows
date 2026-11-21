use boa_string::StaticJsStrings;

use crate::{
    Context, JsResult, JsValue, js_string,
    object::{internal_methods::InternalMethodPropertyContext, shape::slot::SlotAttributes},
    property::PropertyKey,
    vm::opcode::{IndexOperand, Operation, RegisterOperand},
};

fn get_by_name<const LENGTH: bool>(
    (dst, object, receiver, index): (RegisterOperand, &JsValue, &JsValue, IndexOperand),
    context: &mut Context,
) -> JsResult<()> {
    if LENGTH {
        if let Some(object) = object.as_object()
            && object.is_array()
        {
            let value = object.borrow().properties().storage[0].clone();
            context.vm.set_register(dst.into(), value);
            return Ok(());
        } else if let Some(string) = object.as_string() {
            // NOTE: Since we’re using the prototype returned directly by `base_class()`,
            //       we need to handle string primitives separately due to the
            //       string exotic internal methods.
            context
                .vm
                .set_register(dst.into(), (string.len() as u32).into());
            return Ok(());
        }
    }

    // OPTIMIZATION:
    //    Instead of calling `to_object()`, which creates a temporary wrapper object for primitive
    //    values (e.g., numbers, strings, booleans) just to query their prototype chain.
    //
    //    To prevent the creation of a temporary JsObject, we directly retrieve the prototype that
    //    `to_object()` would produce, such as `Number.prototype`, `String.prototype`, etc.
    let object = object.base_class(context)?;

    let ic = &context.vm.frame().code_block().ic[usize::from(index)];
    let object_borrowed = object.borrow();
    // HIWAVE PATCH (ic-exotic-get): a hit only where it is what the object's [[Get]] would do (a
    // Proxy or a typed array can share an ordinary object's shape; see `InlineCache::hit_is_get`).
    if ic.hit_is_get(&object)
        && let Some((shape, slot)) = ic.get(object_borrowed.shape())
    {
        // HIWAVE PATCH (ic-holder): the cached index is read with `get`; a slot that is not
        // there falls through to the full [[Get]] instead of indexing out of bounds.
        let cached = if slot.attributes.contains(SlotAttributes::PROTOTYPE) {
            let prototype = shape.prototype().expect("prototype should have value");
            let prototype = prototype.borrow();
            prototype
                .properties()
                .storage
                .get(slot.index as usize)
                .cloned()
        } else {
            object_borrowed
                .properties()
                .storage
                .get(slot.index as usize)
                .cloned()
        };
        if let Some(mut result) = cached {
            drop(object_borrowed);
            if slot.attributes.has_get() && result.is_object() {
                result = result.as_object().expect("should contain getter").call(
                    receiver,
                    &[],
                    context,
                )?;
            }
            context.vm.set_register(dst.into(), result);
            return Ok(());
        }
    }
    drop(object_borrowed);

    let key: PropertyKey = ic.name.clone().into();

    let context = &mut InternalMethodPropertyContext::new(context);
    let result = object.__get__(&key, receiver.clone(), context)?;

    // Cache the property.
    let slot = *context.slot();
    // HIWAVE PATCH (vm-stats): an inline-cache miss, by receiver kind and name.
    if crate::vm::stats::enabled() {
        crate::vm::stats::miss(
            crate::vm::stats::Site::GetByName,
            &key.to_string(),
            crate::vm::stats::receiver_kind(receiver),
            slot.is_cacheable(),
        );
    }
    if slot.is_cacheable() {
        let ic = &context.vm.frame().code_block.ic[usize::from(index)];
        let object_borrowed = object.borrow();
        let shape = object_borrowed.shape();
        ic.set(shape, slot);
    }

    context.vm.set_register(dst.into(), result);
    Ok(())
}

/// HIWAVE PATCH (js-perf2): the array index a key register denotes when it is a whole number in
/// `0..=2^32-2` (an int32 or a whole double), the only keys the fast paths below take. No clone,
/// no `ToPropertyKey`.
#[inline(always)]
pub(crate) fn fast_index(key: &JsValue) -> Option<u32> {
    use crate::value::JsVariant;
    match key.variant() {
        JsVariant::Integer32(i) if i >= 0 => Some(i as u32),
        JsVariant::Float64(f) if f >= 0.0 && f < 4_294_967_295.0 && f.fract() == 0.0 => Some(f as u32),
        _ => None,
    }
}

/// HIWAVE PATCH (js-perf2): `base[index]` for a dense array element, a typed array element or a
/// string code unit, read straight off the register. `None` means "take the general path" (a hole,
/// a sparse array, any other object). Answers exactly what `__get__` would for these cases: an
/// integer index never consults the prototype of a typed array, and a string primitive's index
/// past its end is `undefined`.
#[inline(always)]
fn fast_get_by_index(base: &JsValue, index: u32) -> JsResult<Option<JsValue>> {
    if let Some(object) = base.as_object() {
        if object.is_array() {
            return Ok(object.borrow().properties().get_dense_property(index));
        }
        if object.is::<crate::builtins::typed_array::TypedArray>() {
            let element = crate::builtins::typed_array::object::typed_array_get_element(&object, f64::from(index))?;
            return Ok(Some(element.unwrap_or_else(JsValue::undefined)));
        }
        return Ok(None);
    }
    if let Some(string) = base.as_string() {
        // Past the end the answer comes from `String.prototype` (`String.prototype[5] = 'P'` makes
        // `'abc'[5]` 'P'), so only an index inside the string is answered here.
        return Ok(string
            .code_unit_at(index as usize)
            .map(|char| js_string!([char].as_slice()).into()));
    }
    Ok(None)
}

fn get_by_value<const PUSH_KEY: bool>(
    (dst, key, receiver, object): (
        RegisterOperand,
        RegisterOperand,
        RegisterOperand,
        RegisterOperand,
    ),
    context: &mut Context,
) -> JsResult<()> {
    // HIWAVE PATCH (js-perf2): the common `a[i]` on an array, typed array or string, before the
    // key and the base are cloned and the key converted.
    if let Some(index) = fast_index(context.vm.get_register(key.into()))
        && let Some(value) = fast_get_by_index(context.vm.get_register(object.into()), index)?
    {
        if PUSH_KEY {
            // As the general path leaves it: the key register holds the key's property-key value.
            let key_value: JsValue = PropertyKey::from(index).into();
            context.vm.set_register(key.into(), key_value);
        }
        context.vm.set_register(dst.into(), value);
        return Ok(());
    }

    let key_value = context.vm.get_register(key.into()).clone();
    let base = context.vm.get_register(object.into()).clone();
    let object = base.base_class(context)?;
    let key_value = key_value.to_property_key(context)?;

    // Fast Path
    //
    // NOTE: Since we’re using the prototype returned directly by `base_class()`,
    //       we need to handle string primitives separately due to the
    //       string exotic internal methods.
    match &key_value {
        PropertyKey::Index(index) => {
            if object.is_array() {
                let object_borrowed = object.borrow();
                if let Some(element) = object_borrowed.properties().get_dense_property(index.get())
                {
                    if PUSH_KEY {
                        context.vm.set_register(key.into(), key_value.into());
                    }

                    context.vm.set_register(dst.into(), element);
                    return Ok(());
                }
            } else if let Some(string) = base.as_string()
                && let Some(char) = string.code_unit_at(index.get() as usize)
            {
                // HIWAVE PATCH (js-perf2): only an index inside the string; past the end the
                // answer comes from `String.prototype` (general path below).
                let value: JsValue = js_string!([char].as_slice()).into();

                if PUSH_KEY {
                    context.vm.set_register(key.into(), key_value.into());
                }
                context.vm.set_register(dst.into(), value);
                return Ok(());
            }
        }
        PropertyKey::String(string) if *string == StaticJsStrings::LENGTH => {
            if let Some(string) = base.as_string() {
                let value = string.len().into();

                if PUSH_KEY {
                    context.vm.set_register(key.into(), key_value.into());
                }
                context.vm.set_register(dst.into(), value);
                return Ok(());
            }
        }
        _ => {}
    }

    let receiver = context.vm.get_register(receiver.into());

    // Slow path:
    let result = object.__get__(
        &key_value,
        receiver.clone(),
        &mut InternalMethodPropertyContext::new(context),
    )?;

    if PUSH_KEY {
        context.vm.set_register(key.into(), key_value.into());
    }
    context.vm.set_register(dst.into(), result);
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct GetLengthProperty;

impl GetLengthProperty {
    #[inline(always)]
    pub(crate) fn operation(
        (dst, object, index): (RegisterOperand, RegisterOperand, IndexOperand),
        context: &mut Context,
    ) -> JsResult<()> {
        let object = context.vm.get_register(object.into()).clone();
        get_by_name::<true>((dst, &object, &object, index), context)
    }
}

impl Operation for GetLengthProperty {
    const NAME: &'static str = "GetLengthProperty";
    const INSTRUCTION: &'static str = "INST - GetLengthProperty";
    const COST: u8 = 4;
}

/// `GetPropertyByName` implements the Opcode Operation for `Opcode::GetPropertyByName`
///
/// Operation:
///  - Get a property by name from an object.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GetPropertyByName;

impl GetPropertyByName {
    #[inline(always)]
    pub(crate) fn operation(
        (dst, object, index): (RegisterOperand, RegisterOperand, IndexOperand),
        context: &mut Context,
    ) -> JsResult<()> {
        let object = context.vm.get_register(object.into()).clone();
        get_by_name::<false>((dst, &object, &object, index), context)
    }
}

impl Operation for GetPropertyByName {
    const NAME: &'static str = "GetPropertyByName";
    const INSTRUCTION: &'static str = "INST - GetPropertyByName";
    const COST: u8 = 4;
}

/// `GetPropertyByNameWithThis` implements the Opcode Operation for `Opcode::GetPropertyByNameWithThis`
///
/// Operation:
///  - Get a property by name from an object with this.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GetPropertyByNameWithThis;

impl GetPropertyByNameWithThis {
    #[inline(always)]
    pub(crate) fn operation(
        (dst, receiver, value, index): (
            RegisterOperand,
            RegisterOperand,
            RegisterOperand,
            IndexOperand,
        ),
        context: &mut Context,
    ) -> JsResult<()> {
        let receiver = context.vm.get_register(receiver.into()).clone();
        let object = context.vm.get_register(value.into()).clone();
        get_by_name::<false>((dst, &object, &receiver, index), context)
    }
}

impl Operation for GetPropertyByNameWithThis {
    const NAME: &'static str = "GetPropertyByNameWithThis";
    const INSTRUCTION: &'static str = "INST - GetPropertyByNameWithThis";
    const COST: u8 = 4;
}

/// `GetPropertyByValue` implements the Opcode Operation for `Opcode::GetPropertyByValue`
///
/// Operation:
///  - Get a property by value from an object and store it in dst.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GetPropertyByValue;

impl GetPropertyByValue {
    #[inline(always)]
    pub(crate) fn operation(
        args: (
            RegisterOperand,
            RegisterOperand,
            RegisterOperand,
            RegisterOperand,
        ),
        context: &mut Context,
    ) -> JsResult<()> {
        get_by_value::<false>(args, context)
    }
}

impl Operation for GetPropertyByValue {
    const NAME: &'static str = "GetPropertyByValue";
    const INSTRUCTION: &'static str = "INST - GetPropertyByValue";
    const COST: u8 = 4;
}

/// `GetPropertyByValuePush` implements the Opcode Operation for `Opcode::GetPropertyByValuePush`
///
/// Operation:
///  - Get a property by value from an object and store the key and value in registers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GetPropertyByValuePush;

impl GetPropertyByValuePush {
    #[inline(always)]
    pub(crate) fn operation(
        args: (
            RegisterOperand,
            RegisterOperand,
            RegisterOperand,
            RegisterOperand,
        ),
        context: &mut Context,
    ) -> JsResult<()> {
        get_by_value::<true>(args, context)
    }
}

impl Operation for GetPropertyByValuePush {
    const NAME: &'static str = "GetPropertyByValuePush";
    const INSTRUCTION: &'static str = "INST - GetPropertyByValuePush";
    const COST: u8 = 4;
}
