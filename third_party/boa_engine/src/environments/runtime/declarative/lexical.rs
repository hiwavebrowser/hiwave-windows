use boa_gc::{Finalize, GcRefCell, Trace};

use crate::JsValue;

#[derive(Debug, Trace, Finalize)]
pub(crate) struct LexicalEnvironment {
    bindings: GcRefCell<Vec<Option<JsValue>>>,
}

impl LexicalEnvironment {
    /// Creates a new `LexicalEnvironment`.
    pub(crate) fn new(bindings: u32) -> Self {
        Self {
            bindings: GcRefCell::new(vec![None; bindings as usize]),
        }
    }

    /// Gets the binding value from the environment by it's index.
    ///
    /// HIWAVE PATCH (eval-var-per-call): `None` for a slot past this environment's end as for an
    /// uninitialized one. An environment created before a direct eval in another call grew the
    /// shared scope is shorter than the indices code compiled after the growth uses.
    pub(crate) fn get(&self, index: u32) -> Option<JsValue> {
        self.bindings.borrow().get(index as usize).cloned().flatten()
    }

    /// Sets the binding value from the environment by index.
    ///
    /// HIWAVE PATCH (eval-var-per-call): a slot past this environment's end (see `get`) is added first.
    pub(crate) fn set(&self, index: u32, value: JsValue) {
        let i = index as usize;
        let mut bindings = self.bindings.borrow_mut();
        if i >= bindings.len() {
            bindings.resize(i + 1, None);
        }
        bindings[i] = Some(value);
    }

    /// Gets the bindings of this poisonable environment.
    #[expect(dead_code)]
    pub(crate) const fn bindings(&self) -> &GcRefCell<Vec<Option<JsValue>>> {
        &self.bindings
    }
}
