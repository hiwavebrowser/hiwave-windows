//! SCRIPT-SPLIT: how much of a script turn was spent inside host functions
//! (the DOM, CSSOM, canvas and other natives the embedder registers with
//! [`JsRuntime::register_host_function`](crate::JsRuntime::register_host_function)),
//! so a stage split can tell the engine's JS execution from the calls it
//! makes across the bindings boundary.
//!
//! Kept per thread, only while [`set_host_call_timing`] is on for it (off
//! by default: a host call then reads one thread-local flag and no clock).
//! Totals are never reset; a caller reads them before and after a turn.
//!
//! A host call's time runs from the native's entry (after the script
//! budget check) to its return, the argument and result conversions
//! included. A host call made from inside another (script the first one
//! ran, an event it dispatched) is its own row's time and not also its
//! caller's ([`HostCallRow::ms`] is self time); the thread total counts
//! the outermost calls only, so script a host call itself runs (an event
//! listener a DOM call dispatched) is host time in the total.
//!
//! Rows are keyed by the host function's name and, for the dispatchers
//! that take an operation name (canvas, DOM writes), that operation: names
//! the embedder's own script passes, cut to an identifier (anything else
//! is `?`). No page content is kept.

#![cfg_attr(not(feature = "boa"), allow(dead_code))]

use std::cell::{Cell, RefCell};
use std::time::Instant;

/// The thread's host calls so far: how many, and the milliseconds the
/// outermost of them took.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HostCallTotals {
    pub calls: u64,
    pub ms: f64,
}

impl HostCallTotals {
    /// What was added since `before` (an earlier read on the same thread).
    pub fn since(&self, before: &Self) -> Self {
        Self { calls: self.calls - before.calls, ms: self.ms - before.ms }
    }
}

/// One host function (and operation): its calls so far and their self
/// time in milliseconds.
#[derive(Debug, Clone, PartialEq)]
pub struct HostCallRow {
    pub name: String,
    /// The operation, for a dispatcher keyed by one; empty otherwise.
    pub op: String,
    pub calls: u64,
    pub ms: f64,
}

#[derive(Default)]
struct Table {
    /// By the host function's slot (see [`slot`]), then by operation (a
    /// few dozen at most: looked up by a scan, never allocated twice).
    rows: Vec<Vec<(Box<str>, u64, f64)>>,
    names: Vec<Box<str>>,
    /// Each open host call's time spent in host calls it made.
    open: Vec<f64>,
}

#[cfg(test)]
thread_local! {
    /// How many operation keys this thread computed (a test's count).
    static OP_KEYS: Cell<u64> = const { Cell::new(0) };
}

thread_local! {
    static TIMING: Cell<bool> = const { Cell::new(false) };
    static TOTALS: Cell<HostCallTotals> = const { Cell::new(HostCallTotals { calls: 0, ms: 0.0 }) };
    static TABLE: RefCell<Table> = RefCell::new(Table::default());
}

/// Keeps this thread's host call totals and rows from now on (`on`), or stops.
pub fn set_host_call_timing(on: bool) {
    TIMING.with(|t| t.set(on));
}

/// This thread's [`HostCallTotals`].
pub fn host_call_totals() -> HostCallTotals {
    TOTALS.with(|t| t.get())
}

/// This thread's rows so far, the most time first.
pub fn host_call_rows() -> Vec<HostCallRow> {
    let mut out: Vec<HostCallRow> = TABLE.with(|t| {
        let t = t.borrow();
        t.rows
            .iter()
            .zip(&t.names)
            .flat_map(|(ops, name)| {
                ops.iter().map(move |&(ref op, calls, ms)| HostCallRow {
                    name: name.to_string(),
                    op: op.to_string(),
                    calls,
                    ms,
                })
            })
            .collect()
    });
    out.sort_by(|a, b| b.ms.total_cmp(&a.ms).then_with(|| (&a.name, &a.op).cmp(&(&b.name, &b.op))));
    out
}

/// `ms` of work done inside the open host call that is not the call's own
/// (a relayout stage the embedder times as a stage of its own, run by a
/// geometry read): taken out of that call's self time and of the thread
/// total. Nothing when no host call is open (or timing is off).
pub fn host_call_exclude(ms: f64) {
    let open = TABLE.with(|t| match t.borrow_mut().open.last_mut() {
        Some(inner) => {
            *inner += ms;
            true
        }
        None => false,
    });
    if open {
        TOTALS.with(|t| {
            let mut v = t.get();
            v.ms -= ms;
            t.set(v);
        });
    }
}

/// What each row added since `before` (an earlier [`host_call_rows`] on
/// the same thread); rows with no calls in between are left out.
pub fn host_call_rows_since(before: &[HostCallRow]) -> Vec<HostCallRow> {
    let mut out: Vec<HostCallRow> = host_call_rows()
        .into_iter()
        .filter_map(|mut row| {
            if let Some(b) = before.iter().find(|b| b.name == row.name && b.op == row.op) {
                row.calls -= b.calls;
                row.ms -= b.ms;
            }
            (row.calls > 0).then_some(row)
        })
        .collect();
    out.sort_by(|a, b| b.ms.total_cmp(&a.ms).then_with(|| (&a.name, &a.op).cmp(&(&b.name, &b.op))));
    out
}

/// The operation key of a dispatcher's argument: up to the first `:`, an
/// identifier of at most 32 characters, else `?`.
pub(crate) fn op_key(op: Option<&str>) -> &str {
    #[cfg(test)]
    OP_KEYS.with(|n| n.set(n.get() + 1));
    let Some(op) = op else { return "" };
    let op = op.split(':').next().unwrap_or("");
    let ident = !op.is_empty() && op.len() <= 32 && op.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if ident {
        op
    } else {
        "?"
    }
}

/// A host function's slot in this thread's table, made when it is
/// registered (a runtime's host functions run on the thread that made it).
pub(crate) fn slot(name: &str) -> usize {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        if let Some(i) = t.names.iter().position(|n| &**n == name) {
            return i;
        }
        t.names.push(name.into());
        t.rows.push(Vec::new());
        t.names.len() - 1
    })
}

/// One host call, timed until dropped when timing is on.
pub(crate) struct HostCall {
    /// When it began, and its slot.
    open: Option<(Instant, usize)>,
    /// Its operation's index in the slot, once named ([`HostCall::op`]).
    index: Option<usize>,
}

/// Opens a host call of `slot`; its clock starts now.
pub(crate) fn enter(slot: usize) -> HostCall {
    if !TIMING.with(|t| t.get()) {
        return HostCall { open: None, index: None };
    }
    TABLE.with(|t| t.borrow_mut().open.push(0.0));
    HostCall { open: Some((Instant::now(), slot)), index: None }
}

/// The index of operation `op` in `slot`, added when new; `None` for a
/// slot this thread never made (a runtime's host functions run on the
/// thread that registered them; such a call counts in the totals only).
fn op_index(t: &mut Table, slot: usize, op: &str) -> Option<usize> {
    let ops = t.rows.get_mut(slot)?;
    Some(match ops.iter().position(|(o, _, _)| &**o == op) {
        Some(i) => i,
        None => {
            ops.push((op.into(), 0, 0.0));
            ops.len() - 1
        }
    })
}

impl HostCall {
    /// Whether this call is timed (the thread's timing was on when it began).
    pub(crate) fn timed(&self) -> bool {
        self.open.is_some()
    }

    /// Names the call's operation (see [`op_key`]); a call never named is
    /// its function's row with no operation.
    pub(crate) fn op(&mut self, op: &str) {
        if let Some((_, slot)) = self.open {
            self.index = TABLE.with(|t| op_index(&mut t.borrow_mut(), slot, op));
        }
    }
}

impl Drop for HostCall {
    fn drop(&mut self) {
        let Some((start, slot)) = self.open else { return };
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let outermost = TABLE.with(|t| {
            let mut t = t.borrow_mut();
            let inner = t.open.pop().unwrap_or(0.0);
            if let Some(parent) = t.open.last_mut() {
                *parent += ms;
            }
            let index = match self.index {
                Some(i) => Some(i),
                None => op_index(&mut t, slot, ""),
            };
            if let Some(row) = index.and_then(|i| t.rows.get_mut(slot)?.get_mut(i)) {
                row.1 += 1;
                row.2 += ms - inner;
            }
            t.open.is_empty()
        });
        TOTALS.with(|t| {
            let mut v = t.get();
            v.calls += 1;
            if outermost {
                v.ms += ms;
            }
            t.set(v);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_operation_key_is_an_identifier_or_a_question_mark() {
        assert_eq!(op_key(None), "");
        assert_eq!(op_key(Some("arc")), "arc");
        assert_eq!(op_key(Some("geom:offset")), "geom");
        assert_eq!(op_key(Some("create:div")), "create");
        assert_eq!(op_key(Some("hello world")), "?");
        assert_eq!(op_key(Some("")), "?");
        assert_eq!(op_key(Some(&"x".repeat(33))), "?");
    }

    fn row<'a>(rows: &'a [HostCallRow], name: &str, op: &str) -> Option<&'a HostCallRow> {
        rows.iter().find(|r| r.name == name && r.op == op)
    }

    /// Script's calls into host functions count per function and, for a
    /// dispatcher, per operation; with timing off nothing is kept.
    #[cfg(feature = "boa")]
    #[test]
    fn host_calls_from_script_count_per_function_and_operation_only_while_timing_is_on() {
        use crate::{JsRuntime, JsValue};
        let mut runtime = JsRuntime::new().unwrap();
        runtime.register_host_function_by_op("__hc_op", 2, 0, Box::new(|_| JsValue::Undefined)).unwrap();
        runtime.register_host_function("__hc_plain", 0, Box::new(|_| JsValue::Number(1.0))).unwrap();
        let script = "for (var i = 0; i < 3; i++) __hc_op('arc', i); __hc_op('fill'); __hc_op('two words'); \
                      __hc_op(7); __hc_plain(); __hc_plain();";
        // Off (the default): nothing kept, and no operation key computed.
        let (rows, totals, keys) = (host_call_rows(), host_call_totals(), OP_KEYS.with(|n| n.get()));
        runtime.evaluate_script(script).unwrap();
        assert_eq!(host_call_totals(), totals);
        assert!(host_call_rows_since(&rows).is_empty());
        assert_eq!(OP_KEYS.with(|n| n.get()), keys, "an operation key was computed with timing off");
        set_host_call_timing(true);
        let (rows, totals) = (host_call_rows(), host_call_totals());
        runtime.evaluate_script(script).unwrap();
        let d = host_call_totals().since(&totals);
        let rows = host_call_rows_since(&rows);
        set_host_call_timing(false);
        assert_eq!(d.calls, 8, "{rows:?}");
        assert!(d.ms >= 0.0);
        let calls = |name, op| row(&rows, name, op).map(|r| r.calls);
        assert_eq!(calls("__hc_op", "arc"), Some(3));
        assert_eq!(calls("__hc_op", "fill"), Some(1));
        assert_eq!(calls("__hc_op", "?"), Some(1), "an operation that is no identifier");
        assert_eq!(calls("__hc_op", ""), Some(1), "no operation string");
        assert_eq!(calls("__hc_plain", ""), Some(2));
        assert_eq!(rows.len(), 5, "{rows:?}");
        let sum: f64 = rows.iter().map(|r| r.ms).sum();
        assert!((sum - d.ms).abs() < 1e-9, "rows {sum} vs total {}", d.ms);
    }

    /// A host call inside another is its own row's time, not also its
    /// caller's; the thread total counts the outermost call once. Work the
    /// embedder excludes (a relayout stage) leaves both. Sums, not bounds.
    #[test]
    fn a_nested_host_call_is_self_time_and_excluded_work_leaves_the_call() {
        let (outer, inner) = (slot("__hc_outer"), slot("__hc_inner"));
        set_host_call_timing(true);
        let (rows, totals) = (host_call_rows(), host_call_totals());
        {
            let mut a = enter(outer);
            a.op("a");
            for _ in 0..1000 {
                std::hint::black_box(0);
            }
            {
                let _b = enter(inner);
                for _ in 0..1000 {
                    std::hint::black_box(0);
                }
            }
        }
        let d = host_call_totals().since(&totals);
        let r = host_call_rows_since(&rows);
        let (o, i) = (row(&r, "__hc_outer", "a").unwrap(), row(&r, "__hc_inner", "").unwrap());
        assert_eq!((d.calls, o.calls, i.calls), (2, 1, 1));
        assert!((o.ms + i.ms - d.ms).abs() < 1e-9, "self times {} + {} vs the outermost {}", o.ms, i.ms, d.ms);
        // Excluded inside: a synthetic 1 s, more than the call took.
        let (rows, totals) = (host_call_rows(), host_call_totals());
        {
            let _a = enter(outer);
            host_call_exclude(1000.0);
        }
        let d = host_call_totals().since(&totals);
        let r = host_call_rows_since(&rows);
        assert!(d.ms < 0.0 && row(&r, "__hc_outer", "").unwrap().ms < 0.0, "{d:?} {r:?}");
        // No call open: nothing to take it from.
        let totals = host_call_totals();
        host_call_exclude(5.0);
        assert_eq!(host_call_totals(), totals);
        set_host_call_timing(false);
    }
}
