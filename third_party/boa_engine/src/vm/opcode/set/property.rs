use crate::JsExpect;
use crate::JsValue;
use crate::object::JsObject;
use crate::value::JsVariant;
use crate::vm::opcode::{IndexOperand, RegisterOperand};
use crate::{
    Context, JsNativeError, JsResult,
    builtins::function::set_function_name,
    object::{internal_methods::InternalMethodPropertyContext, shape::slot::SlotAttributes},
    property::{PropertyDescriptor, PropertyKey},
    vm::opcode::Operation,
};
use boa_macros::js_str;
use crate::string::StaticJsStrings;

/// HIWAVE PATCH (array-length-ic): whether `object` defines properties the ordinary way, the
/// only case in which `SetPropertyByName` may write a cached slot directly.
#[inline(always)]
fn has_ordinary_define_own_property(object: &JsObject) -> bool {
    let ordinary: fn(
        &JsObject,
        &PropertyKey,
        PropertyDescriptor,
        &mut InternalMethodPropertyContext<'_>,
    ) -> JsResult<bool> = crate::object::internal_methods::ordinary_define_own_property;
    std::ptr::fn_addr_eq(object.vtable().__define_own_property__, ordinary)
}

/// HIWAVE PATCH (super-set-receiver): whether `object`'s [[Set]] is the ordinary one, the only case
/// in which a cached prototype setter is what [[Set]] would call. A proxy (its `set` trap) and a
/// typed array (a canonical numeric key such as `Infinity` never reaches the prototype) can share
/// an ordinary object's shape; a String wrapper keeps the ordinary [[Set]].
#[inline(always)]
fn has_ordinary_set(object: &JsObject) -> bool {
    let ordinary: fn(
        &JsObject,
        PropertyKey,
        JsValue,
        JsValue,
        &mut InternalMethodPropertyContext<'_>,
    ) -> JsResult<bool> = crate::object::internal_methods::ordinary_set;
    std::ptr::fn_addr_eq(object.vtable().__set__, ordinary)
}

fn set_by_name(
    value: RegisterOperand,
    value_object: &JsValue,
    receiver: &JsValue,
    index: IndexOperand,
    context: &mut Context,
) -> JsResult<()> {
    let value = context.vm.get_register(value.into()).clone();

    let object = value_object.to_object(context)?;

    let ic = &context.vm.frame().code_block().ic[usize::from(index)];

    // HIWAVE PATCH (super-set-receiver): whether [[Set]] would write the object's own storage, i.e.
    // the receiver is the object itself. `SetPropertyByName` passes one value as both, so that is a
    // pointer comparison; a primitive base never is (the object is a fresh wrapper), and neither is
    // `super.x = v` with `this` other than the home object's prototype.
    let receiver_is_object = if std::ptr::eq(value_object, receiver) {
        value_object.is_object()
    } else {
        receiver.as_object().is_some_and(|r| JsObject::equals(&r, &object))
    };

    let object_borrowed = object.borrow();
    if let Some((shape, slot)) = ic.get(object_borrowed.shape())
        // HIWAVE PATCH (super-set-receiver): an accessor hit calls the setter with the receiver, so
        // it is right for any receiver, on an object whose [[Set]] is the ordinary one (a proxy or
        // a typed array sharing the shape would skip its own [[Set]]). A data hit writes a slot in
        // place, which is what [[Set]] does only for an own data property of the receiver: the
        // slot must be the object's own (not `PROTOTYPE`: a data property found on the prototype
        // is defined on the receiver, never written where it was found) and the receiver must be
        // the object.
        && ((slot.attributes.is_accessor_descriptor() && has_ordinary_set(&object))
            || (!slot.attributes.contains(SlotAttributes::PROTOTYPE)
                && receiver_is_object
                // HIWAVE PATCH (array-length-ic): and only on an object whose [[DefineOwnProperty]]
                // is the ordinary one, or on an array for any name but `length`. An Array subclass
                // instance has the same shape as `Object.create(Q.prototype)` given a writable own
                // `length`, so a site that cached that object's `length` would otherwise write a
                // real array's `length` past ArraySetLength.
                && (has_ordinary_define_own_property(&object)
                    || (object.is_array() && ic.name != StaticJsStrings::LENGTH))))
    {
        let slot_index = slot.index as usize;

        // HIWAVE PATCH (ic-holder): every cached index is read with `get`, and a hit that finds no
        // slot, or no callable setter, falls through to the full [[Set]] below instead of
        // indexing out of bounds or returning as if a setter had run. A setter can become
        // `undefined` without a shape change (accessor -> accessor keeps the slot's bits), and
        // the full path is what throws for that in strict code.
        if slot.attributes.is_accessor_descriptor() {
            let setter = if slot.attributes.contains(SlotAttributes::PROTOTYPE) {
                let prototype = shape.prototype().expect("prototype should have value");
                let prototype = prototype.borrow();
                prototype.properties().storage.get(slot_index + 1).cloned()
            } else {
                object_borrowed
                    .properties()
                    .storage
                    .get(slot_index + 1)
                    .cloned()
            };

            if slot.attributes.has_set()
                && let Some(setter) = setter.as_ref().and_then(JsValue::as_object)
            {
                drop(object_borrowed);
                setter.call(receiver, std::slice::from_ref(&value), context)?;
                return Ok(());
            }
        } else if slot_index < object_borrowed.properties().storage.len() {
            // HIWAVE PATCH (super-set-receiver): a data hit is always the object's own slot (the
            // guard above); upstream's branch writing a prototype's data slot in place is gone.
            // (ic-holder: and the index is checked; a slot that is not there takes the full path.)
            debug_assert!(!slot.attributes.contains(SlotAttributes::PROTOTYPE));
            drop(object_borrowed);
            let mut object_borrowed = object.borrow_mut();
            object_borrowed.properties_mut().storage[slot_index] = value.clone();
            return Ok(());
        }
    }
    drop(object_borrowed);

    let name: PropertyKey = ic.name.clone().into();

    let context = &mut InternalMethodPropertyContext::new(context);
    let succeeded = object.__set__(name.clone(), value.clone(), receiver.clone(), context)?;
    if !succeeded && context.vm.frame().code_block.strict() {
        return Err(JsNativeError::typ()
            .with_message(format!("cannot set non-writable property: {name}"))
            .into());
    }

    // Cache the property.
    let slot = *context.slot();
    // HIWAVE PATCH (vm-stats): an inline-cache miss, by receiver kind and name.
    if crate::vm::stats::enabled() {
        crate::vm::stats::miss(
            crate::vm::stats::Site::SetByName,
            &name.to_string(),
            crate::vm::stats::receiver_kind(receiver),
            succeeded && slot.is_cacheable(),
        );
    }
    // HIWAVE PATCH (super-set-receiver): when the receiver is not the object, a data slot recorded
    // here may describe the receiver's define, not the object's storage, so only an accessor slot
    // (found on the object's chain) is kept.
    if succeeded
        && slot.is_cacheable()
        && (receiver_is_object || slot.attributes.is_accessor_descriptor())
    {
        let ic = &context.vm.frame().code_block.ic[usize::from(index)];
        let object_borrowed = object.borrow();
        let shape = object_borrowed.shape();
        ic.set(shape, slot);
    }

    Ok(())
}

/// `SetPropertyByName` implements the Opcode Operation for `Opcode::SetPropertyByName`
///
/// Operation:
///  - Sets a property by name of an object.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SetPropertyByName;

impl SetPropertyByName {
    #[inline(always)]
    pub(crate) fn operation(
        (value, object, index): (RegisterOperand, RegisterOperand, IndexOperand),
        context: &mut Context,
    ) -> JsResult<()> {
        let object = context.vm.get_register(object.into()).clone();
        set_by_name(value, &object, &object, index, context)
    }
}

impl Operation for SetPropertyByNameWithThis {
    const NAME: &'static str = "SetPropertyByNameWithThis";
    const INSTRUCTION: &'static str = "INST - SetPropertyByNameWithThis";
    const COST: u8 = 4;
}

/// `SetPropertyByNameWithThis` implements the Opcode Operation for `Opcode::SetPropertyByNameWithThis`
///
/// Operation:
///  - Sets a property by name of an object.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SetPropertyByNameWithThis;

impl SetPropertyByNameWithThis {
    #[inline(always)]
    pub(crate) fn operation(
        (value, receiver, object, index): (
            RegisterOperand,
            RegisterOperand,
            RegisterOperand,
            IndexOperand,
        ),
        context: &mut Context,
    ) -> JsResult<()> {
        let value_object = context.vm.get_register(object.into()).clone();
        let receiver = context.vm.get_register(receiver.into()).clone();
        set_by_name(value, &value_object, &receiver, index, context)
    }
}

impl Operation for SetPropertyByName {
    const NAME: &'static str = "SetPropertyByName";
    const INSTRUCTION: &'static str = "INST - SetPropertyByName";
    const COST: u8 = 4;
}

/// `SetPropertyByValue` implements the Opcode Operation for `Opcode::SetPropertyByValue`
///
/// Operation:
///  - Sets a property by value of an object.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SetPropertyByValue;

impl SetPropertyByValue {
    #[inline(always)]
    pub(crate) fn operation(
        (value, key, receiver, object): (
            RegisterOperand,
            RegisterOperand,
            RegisterOperand,
            RegisterOperand,
        ),
        context: &mut Context,
    ) -> JsResult<()> {
        // HIWAVE PATCH (js-perf2): `a[i] = v` on an extensible array's dense element, or a typed
        // array's element (an ordinary assignment, where the receiver is the base itself), before
        // anything is cloned and the key converted.
        if let Some(index) = crate::vm::opcode::get::fast_index(context.vm.get_register(key.into()))
            && let Some(base) = context.vm.get_register(object.into()).as_object()
        {
            // The receiver must be the base itself: `super[i] = v` on an array home object, or
            // `Reflect.set(arr, i, v, other)`, defines the element on the receiver instead.
            if base.is_array()
                && context
                    .vm
                    .get_register(receiver.into())
                    .as_object()
                    .is_some_and(|r| JsObject::equals(&r, &base))
            {
                let mut base_borrowed = base.borrow_mut();
                if base_borrowed.extensible
                    && base_borrowed
                        .properties_mut()
                        .set_dense_property(index, context.vm.get_register(value.into()))
                {
                    return Ok(());
                }
            } else if base.is::<crate::builtins::typed_array::TypedArray>()
                && context
                    .vm
                    .get_register(receiver.into())
                    .as_object()
                    .is_some_and(|r| JsObject::equals(&r, &base))
            {
                let value = context.vm.get_register(value.into()).clone();
                crate::builtins::typed_array::object::typed_array_set_element(
                    &base,
                    f64::from(index),
                    &value,
                    &mut context.into(),
                )?;
                return Ok(());
            }
        }

        let value = context.vm.get_register(value.into()).clone();
        let key = context.vm.get_register(key.into()).clone();
        let receiver = context.vm.get_register(receiver.into()).clone();
        let object = context.vm.get_register(object.into()).clone();
        let object = object.to_object(context)?;

        let key = key.to_property_key(context)?;

        // Fast Path:
        'fast_path: {
            // HIWAVE PATCH (js-perf2): only when the receiver is the array itself.
            if object.is_array()
                && receiver.as_object().is_some_and(|r| JsObject::equals(&r, &object))
                && let PropertyKey::Index(index) = &key
            {
                let mut object_borrowed = object.borrow_mut();

                // Cannot modify if not extensible.
                if !object_borrowed.extensible {
                    break 'fast_path;
                }

                if object_borrowed
                    .properties_mut()
                    .set_dense_property(index.get(), &value)
                {
                    return Ok(());
                }
            }
        }

        // Slow path:
        let succeeded = object.__set__(
            key.clone(),
            value.clone(),
            receiver.clone(),
            &mut context.into(),
        )?;
        if !succeeded && context.vm.frame().code_block.strict() {
            return Err(JsNativeError::typ()
                .with_message(format!("cannot set non-writable property: {key}"))
                .into());
        }

        Ok(())
    }
}

impl Operation for SetPropertyByValue {
    const NAME: &'static str = "SetPropertyByValue";
    const INSTRUCTION: &'static str = "INST - SetPropertyByValue";
    const COST: u8 = 4;
}

/// `SetPropertyGetterByName` implements the Opcode Operation for `Opcode::SetPropertyGetterByName`
///
/// Operation:
///  - Sets a getter property by name of an object.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SetPropertyGetterByName;

impl SetPropertyGetterByName {
    #[inline(always)]
    pub(crate) fn operation(
        (object, value, index): (RegisterOperand, RegisterOperand, IndexOperand),
        context: &mut Context,
    ) -> JsResult<()> {
        let object = context.vm.get_register(object.into()).clone();
        let value = context.vm.get_register(value.into()).clone();
        let name = context
            .vm
            .frame()
            .code_block()
            .constant_string(index.into())
            .into();

        let object = object.to_object(context)?;
        let set = object
            .__get_own_property__(&name, &mut InternalMethodPropertyContext::new(context))?
            .as_ref()
            .and_then(PropertyDescriptor::set)
            .cloned();
        object.__define_own_property__(
            &name,
            PropertyDescriptor::builder()
                .maybe_get(Some(value.clone()))
                .maybe_set(set)
                .enumerable(true)
                .configurable(true)
                .build(),
            &mut InternalMethodPropertyContext::new(context),
        )?;
        Ok(())
    }
}

impl Operation for SetPropertyGetterByName {
    const NAME: &'static str = "SetPropertyGetterByName";
    const INSTRUCTION: &'static str = "INST - SetPropertyGetterByName";
    const COST: u8 = 4;
}

/// `SetPropertyGetterByValue` implements the Opcode Operation for `Opcode::SetPropertyGetterByValue`
///
/// Operation:
///  - Sets a getter property by value of an object.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SetPropertyGetterByValue;

impl SetPropertyGetterByValue {
    #[inline(always)]
    pub(crate) fn operation(
        (value, key, object): (RegisterOperand, RegisterOperand, RegisterOperand),
        context: &mut Context,
    ) -> JsResult<()> {
        let value = context.vm.get_register(value.into()).clone();
        let key = context.vm.get_register(key.into()).clone();
        let object = context.vm.get_register(object.into()).clone();
        let object = object.to_object(context)?;
        let name = key.to_property_key(context)?;

        let set = object
            .__get_own_property__(&name, &mut InternalMethodPropertyContext::new(context))?
            .as_ref()
            .and_then(PropertyDescriptor::set)
            .cloned();
        object.__define_own_property__(
            &name,
            PropertyDescriptor::builder()
                .maybe_get(Some(value.clone()))
                .maybe_set(set)
                .enumerable(true)
                .configurable(true)
                .build(),
            &mut InternalMethodPropertyContext::new(context),
        )?;
        Ok(())
    }
}

impl Operation for SetPropertyGetterByValue {
    const NAME: &'static str = "SetPropertyGetterByValue";
    const INSTRUCTION: &'static str = "INST - SetPropertyGetterByValue";
    const COST: u8 = 4;
}

/// `SetPropertySetterByName` implements the Opcode Operation for `Opcode::SetPropertySetterByName`
///
/// Operation:
///  - Sets a setter property by name of an object.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SetPropertySetterByName;

impl SetPropertySetterByName {
    #[inline(always)]
    pub(crate) fn operation(
        (object, value, index): (RegisterOperand, RegisterOperand, IndexOperand),
        context: &mut Context,
    ) -> JsResult<()> {
        let object = context.vm.get_register(object.into()).clone();
        let value = context.vm.get_register(value.into()).clone();
        let name = context
            .vm
            .frame()
            .code_block()
            .constant_string(index.into())
            .into();

        let object = object.to_object(context)?;

        let get = object
            .__get_own_property__(&name, &mut InternalMethodPropertyContext::new(context))?
            .as_ref()
            .and_then(PropertyDescriptor::get)
            .cloned();
        object.__define_own_property__(
            &name,
            PropertyDescriptor::builder()
                .maybe_set(Some(value.clone()))
                .maybe_get(get)
                .enumerable(true)
                .configurable(true)
                .build(),
            &mut InternalMethodPropertyContext::new(context),
        )?;
        Ok(())
    }
}

impl Operation for SetPropertySetterByName {
    const NAME: &'static str = "SetPropertySetterByName";
    const INSTRUCTION: &'static str = "INST - SetPropertySetterByName";
    const COST: u8 = 4;
}

/// `SetPropertySetterByValue` implements the Opcode Operation for `Opcode::SetPropertySetterByValue`
///
/// Operation:
///  - Sets a setter property by value of an object.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SetPropertySetterByValue;

impl SetPropertySetterByValue {
    #[inline(always)]
    pub(crate) fn operation(
        (value, key, object): (RegisterOperand, RegisterOperand, RegisterOperand),
        context: &mut Context,
    ) -> JsResult<()> {
        let value = context.vm.get_register(value.into()).clone();
        let key = context.vm.get_register(key.into()).clone();
        let object = context.vm.get_register(object.into()).clone();

        let object = object.to_object(context)?;
        let name = key.to_property_key(context)?;

        let get = object
            .__get_own_property__(&name, &mut InternalMethodPropertyContext::new(context))?
            .as_ref()
            .and_then(PropertyDescriptor::get)
            .cloned();
        object.__define_own_property__(
            &name,
            PropertyDescriptor::builder()
                .maybe_set(Some(value.clone()))
                .maybe_get(get)
                .enumerable(true)
                .configurable(true)
                .build(),
            &mut InternalMethodPropertyContext::new(context),
        )?;
        Ok(())
    }
}

impl Operation for SetPropertySetterByValue {
    const NAME: &'static str = "SetPropertySetterByValue";
    const INSTRUCTION: &'static str = "INST - SetPropertySetterByValue";
    const COST: u8 = 4;
}

/// `SetFunctionName` implements the Opcode Operation for `Opcode::SetFunctionName`
///
/// Operation:
///  - Sets the name of a function object.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SetFunctionName;

impl SetFunctionName {
    #[inline(always)]
    pub(crate) fn operation(
        (function, name, prefix): (RegisterOperand, RegisterOperand, IndexOperand),
        context: &mut Context,
    ) -> JsResult<()> {
        let function = context.vm.get_register(function.into()).clone();
        let name = context.vm.get_register(name.into()).clone();
        let name = match name.variant() {
            JsVariant::String(name) => PropertyKey::from(name.clone()),
            JsVariant::Symbol(name) => PropertyKey::from(name.clone()),
            _ => unreachable!(),
        };

        let prefix = match u32::from(prefix) {
            1 => Some(js_str!("get")),
            2 => Some(js_str!("set")),
            _ => None,
        };

        set_function_name(
            &function
                .as_object()
                .js_expect("function is not an object")?,
            &name,
            prefix,
            context,
        )?;
        Ok(())
    }
}

impl Operation for SetFunctionName {
    const NAME: &'static str = "SetFunctionName";
    const INSTRUCTION: &'static str = "INST - SetFunctionName";
    const COST: u8 = 4;
}
