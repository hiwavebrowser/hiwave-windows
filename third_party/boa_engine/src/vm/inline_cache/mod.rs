use arrayvec::ArrayVec;
use itertools::Itertools;
use std::{cell::Cell, fmt};

use boa_gc::GcRefCell;
use boa_macros::{Finalize, Trace};

use crate::{
    JsString,
    object::shape::{
        Shape, WeakShape,
        slot::{Slot, SlotAttributes},
    },
    property::PropertyKey,
};

#[cfg(test)]
mod tests;

pub(crate) const PIC_CAPACITY: usize = 4;

/// A cached shape-to-slot mapping for a polymorphic inline cache.
#[derive(Clone, Debug, Trace, Finalize)]
pub(crate) struct CacheEntry {
    /// A weak reference is kept to the shape to avoid the shape preventing deallocation.
    pub(crate) shape: WeakShape,
    // HIWAVE PATCH (ic-holder): for a slot found on the receiver's prototype, the prototype's
    // shape when the entry was made. The entry is keyed on the receiver's shape alone, which does
    // not change when the prototype's properties do; a hit also requires the prototype's shape now
    // to be this one, so a deleted or redefined prototype property (which gives the prototype a
    // new shape) misses instead of indexing a slot that has moved or gone. Held strongly, so its
    // address cannot be reused by another shape while the entry exists, and the check on a hit
    // is one address comparison; a stale entry is dropped on its next lookup.
    pub(crate) holder: Option<Shape>,
    #[unsafe_ignore_trace]
    pub(crate) slot: Slot,
}

/// An inline cache entry for a property access.
#[derive(Clone, Debug, Trace, Finalize)]
pub(crate) struct InlineCache {
    /// The property that is accessed.
    pub(crate) name: JsString,

    /// Multiple cached shape-to-slot entries.
    pub(crate) entries: GcRefCell<ArrayVec<CacheEntry, PIC_CAPACITY>>,

    /// Whether this access site has seen too many shapes and should no longer be cached.
    #[unsafe_ignore_trace]
    pub(crate) megamorphic: Cell<bool>,

    /// HIWAVE PATCH (ic-exotic-get): whether `name` is a canonical numeric string (an array index,
    /// `Infinity`, `NaN`, `-0`, `1.5`, ...), the names for which a typed array's [[Get]] reads its
    /// elements and never the prototype, and the only names a mapped arguments object's [[Get]]
    /// can answer from its parameter map. Worked out once here so a hit does not parse the name.
    #[unsafe_ignore_trace]
    pub(crate) numeric_name: bool,
}

impl fmt::Display for InlineCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(name:{} entries:", self.name.display_escaped())?;

        if self.megamorphic.get() {
            return write!(f, "(megamorphic))");
        }

        let entries = self.entries.borrow();
        let entries = entries.iter().map(|e| e.shape.to_addr_usize()).format(", ");

        write!(f, "({entries:#x}))")
    }
}

impl InlineCache {
    pub(crate) fn new(name: JsString) -> Self {
        // A canonical numeric string is ToString of a Number: it starts with a digit, `-`, `I`
        // (`Infinity`) or `N` (`NaN`). Every other name, nearly all of them, skips the parse.
        let numeric_name = name
            .code_unit_at(0)
            .is_some_and(|c| matches!(c, 0x30..=0x39 | 0x2D | 0x49 | 0x4E))
            && crate::builtins::typed_array::object::canonical_numeric_index_string(&name)
                .is_some();
        Self {
            name,
            entries: GcRefCell::new(ArrayVec::new()),
            megamorphic: Cell::new(false),
            numeric_name,
        }
    }

    /// HIWAVE PATCH (ic-exotic-get): whether a read that hits this cache on `object` is what
    /// `object`'s [[Get]] would do. The cache is keyed on shape, and an exotic object can share an
    /// ordinary object's shape (a Proxy made from `{}`, a typed array and an object created from its
    /// prototype). An ordinary [[Get]] always is; a typed array's and an arguments object's are for
    /// a name that is not numeric (they differ from the ordinary one only for numeric names); a
    /// Proxy's (its `get` trap) and a module namespace's never are.
    #[inline(always)]
    pub(crate) fn hit_is_get(&self, object: &crate::JsObject) -> bool {
        type Get = fn(
            &crate::JsObject,
            &PropertyKey,
            crate::JsValue,
            &mut crate::object::internal_methods::InternalMethodPropertyContext<'_>,
        ) -> crate::JsResult<crate::JsValue>;
        let get = object.vtable().__get__;
        let ordinary: Get = crate::object::internal_methods::ordinary_get;
        if std::ptr::fn_addr_eq(get, ordinary) {
            return true;
        }
        let typed_array: Get = crate::builtins::typed_array::object::typed_array_exotic_get;
        let arguments: Get = crate::builtins::function::arguments::arguments_exotic_get;
        !self.numeric_name
            && (std::ptr::fn_addr_eq(get, typed_array) || std::ptr::fn_addr_eq(get, arguments))
    }

    pub(crate) fn set(&self, shape: &Shape, slot: Slot) {
        if self.megamorphic.get() {
            return;
        }

        // HIWAVE PATCH (ic-holder): a prototype slot is cached with the prototype's shape (see
        // `CacheEntry::holder`). Not on a receiver in dictionary mode: a unique shape takes a new
        // own property in place, without a new shape, so an entry keyed on it would keep answering
        // from the prototype after the receiver gains an own property that shadows it.
        let holder = if slot.attributes.contains(SlotAttributes::PROTOTYPE) {
            if shape.is_unique() {
                return;
            }
            let Some(prototype) = shape.prototype() else {
                return;
            };
            let prototype = prototype.borrow();
            Some(prototype.shape().clone())
        } else {
            None
        };

        // HIWAVE PATCH (ic-holder): the slot was taken when the lookup found the property, but
        // the shapes are taken now, after any getter or setter it ran; one that redefined or
        // deleted its own property leaves a slot that the current shape no longer has there. Cache
        // only if the name still resolves to that slot, with the same attributes, in the shape of
        // the object that holds it.
        let key = PropertyKey::from(self.name.clone());
        let found = holder.as_ref().unwrap_or(shape).lookup(&key);
        let same = |a: SlotAttributes| a.difference(SlotAttributes::INLINE_CACHE_BITS);
        if !found.is_some_and(|found| {
            found.index == slot.index && same(found.attributes) == same(slot.attributes)
        }) {
            return;
        }
        // HIWAVE PATCH (ic-holder): and for a prototype slot, the receiver must still lack the
        // name. A prototype accessor that defines it on the receiver (a memoising getter) gives the
        // receiver a new shape that has it; an entry keyed on that shape would skip the own property.
        if holder.is_some() && shape.lookup(&key).is_some() {
            return;
        }

        let mut entries = self.entries.borrow_mut();

        // HIWAVE PATCH (ic-set-dedupe): one entry per shape. A site that is filled again for a
        // shape it already has (a getter or setter that runs the same site, once per level of
        // recursion; or any fill that follows a hit that fell through) updates that entry. Pushed
        // as new, five fills of one shape would use all four slots and make the site megamorphic
        // for every receiver.
        let shape_addr = shape.to_addr_usize();
        if let Some(entry) = entries
            .iter_mut()
            .find(|e| e.shape.upgrade().is_some_and(|s| s.to_addr_usize() == shape_addr))
        {
            entry.slot = slot;
            entry.holder = holder;
            return;
        }

        // Add a new entry if there's space.
        if entries
            .try_push(CacheEntry {
                shape: shape.into(),
                holder,
                slot,
            })
            .is_err()
        {
            // Polymorphic cache is full, transition to megamorphic.
            self.megamorphic.set(true);
            entries.clear();
        }
    }

    /// Returns the cached `(Shape, Slot)` if a matching shape exists in the inline cache.
    ///
    /// Opportunistically cleans up stale weak shape references during lookup.
    pub(crate) fn get(&self, shape: &Shape) -> Option<(Shape, Slot)> {
        if self.megamorphic.get() {
            return None;
        }

        let mut entries = self.entries.borrow_mut();
        let mut i = 0;
        let mut result = None;
        let shape_addr = shape.to_addr_usize();

        while i < entries.len() {
            if let Some(upgraded) = entries[i].shape.upgrade() {
                if upgraded.to_addr_usize() == shape_addr {
                    // HIWAVE PATCH (ic-holder): a prototype entry hits only while the prototype
                    // still has the shape it had when the entry was made; otherwise the entry is
                    // stale for good and goes.
                    if let Some(holder) = &entries[i].holder {
                        let fresh = upgraded.prototype().is_some_and(|prototype| {
                            holder.to_addr_usize() == prototype.borrow().shape().to_addr_usize()
                        });
                        if !fresh {
                            entries.swap_remove(i);
                            break;
                        }
                    }
                    result = Some((upgraded, entries[i].slot));
                    break;
                }
                i += 1;
            } else {
                // Opportunistically clean up stale weak shapes.
                entries.swap_remove(i);
            }
        }

        result
    }
}
