//! HIWAVE PATCH (vm-stats). Diagnostic counters for the interpreter, off unless the environment
//! has `RUSTKIT_VM_STATS=1` (read once per process). When on, the run loop counts every opcode it
//! executes, and the property opcodes count their inline-cache MISSES by site, receiver kind and
//! property name; hits are the opcode count minus the misses. When off, the run loop is the
//! unchanged one (`run_impl::<false>` compiles the counting out) and the miss hooks are a single
//! branch on a path that is already the slow one.
//!
//! `report()` returns the text; with `RUSTKIT_VM_STATS_FILE=<path>` the report is also rewritten to
//! `<path>.t<thread number>` every 1M opcodes and when the thread ends, so a probe that exits without calling
//! `report()` still leaves one.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// HIWAVE PATCH (regex-literal-cache): how many times a regular expression pattern was compiled to
/// a matcher in this process. Always counted (one relaxed increment next to a pattern compile).
static REGEXP_COMPILES: AtomicU64 = AtomicU64::new(0);

pub(crate) fn count_regexp_compile() {
    REGEXP_COMPILES.fetch_add(1, Ordering::Relaxed);
}

/// The number of regular expression pattern compiles so far in this process. Diagnostic and tests.
#[must_use]
pub fn regexp_compiles() -> u64 {
    REGEXP_COMPILES.load(Ordering::Relaxed)
}

/// Whether the counters are on in this process.
pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUSTKIT_VM_STATS").is_some_and(|v| !v.is_empty() && v != "0"))
}

/// Where an inline-cache miss happened.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Site {
    GetByName,
    SetByName,
    GetGlobal,
}

impl Site {
    fn name(self) -> &'static str {
        match self {
            Self::GetByName => "GetPropertyByName",
            Self::SetByName => "SetPropertyByName",
            Self::GetGlobal => "GetNameGlobal",
        }
    }
}

const FLUSH_EVERY: u64 = 1 << 20;
const MAX_NAMES: usize = 4096;

struct Stats {
    ops: [u64; 256],
    total: u64,
    /// (site, cacheable after the miss) -> count.
    misses: HashMap<(Site, bool), u64>,
    /// (site, receiver kind, name) -> count, capped at `MAX_NAMES` distinct keys.
    names: HashMap<(Site, &'static str, String), u64>,
}

impl Default for Stats {
    fn default() -> Self {
        Self { ops: [0; 256], total: 0, misses: HashMap::new(), names: HashMap::new() }
    }
}

impl Drop for Stats {
    fn drop(&mut self) {
        if self.total > 0 {
            write_file(&render(self));
        }
    }
}

thread_local! {
    static STATS: RefCell<Stats> = RefCell::new(Stats::default());
}

/// One opcode executed (the counting run loop calls this before each instruction).
#[inline]
pub(crate) fn op(byte: u8) {
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        s.ops[byte as usize] += 1;
        s.total += 1;
        if s.total % FLUSH_EVERY == 0 {
            write_file(&render(&s));
        }
    });
}

/// An inline-cache miss: `receiver` is the kind of value the access was made on, `cacheable` whether
/// the lookup could be cached afterwards (false: the property is not cacheable at all, so every
/// execution will miss).
pub(crate) fn miss(site: Site, name: &str, receiver: &'static str, cacheable: bool) {
    if !enabled() {
        return;
    }
    let name = shape_of(name);
    let name = name.as_str();
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        *s.misses.entry((site, cacheable)).or_insert(0) += 1;
        if s.names.len() < MAX_NAMES || s.names.contains_key(&(site, receiver, name.to_string())) {
            *s.names.entry((site, receiver, name.to_string())).or_insert(0) += 1;
        }
    });
}

/// Privacy: logs and reports carry shapes, not data. A property name is recorded only when it is an
/// identifier of reasonable length, which is what a name written in the page's script source is;
/// anything else (a string-literal key such as an email or a token, an over-long name) is counted
/// under one bucket. Computed keys (`o[x]`) never reach this: the miss hooks are on the by-name
/// opcodes only.
const OTHER_NAME: &str = "(not an identifier, or longer than 48)";

fn shape_of(name: &str) -> String {
    let mut chars = name.chars();
    let ok = name.len() <= 48
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if ok { name.to_string() } else { OTHER_NAME.to_string() }
}

/// The kind of value an access is made on, for the miss report.
pub(crate) fn receiver_kind(value: &crate::JsValue) -> &'static str {
    if value.is_string() {
        "string"
    } else if value.is_number() {
        "number"
    } else if let Some(object) = value.as_object() {
        if object.is_array() {
            "array"
        } else if object.is_callable() {
            "function"
        } else {
            "object"
        }
    } else {
        "other"
    }
}

/// One file per thread (`<path>.t<N>`), so two threads' reports do not overwrite each other.
fn write_file(text: &str) {
    if let Some(path) = std::env::var_os("RUSTKIT_VM_STATS_FILE") {
        let id = format!("{:?}", std::thread::current().id());
        let digits: String = id.chars().filter(char::is_ascii_digit).collect();
        let mut path = path;
        path.push(format!(".t{digits}"));
        drop(std::fs::write(path, text));
    }
}

fn render(s: &Stats) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "vm stats: {} opcodes executed", s.total);
    let mut ops: Vec<(u8, u64)> = s.ops.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, n)| (i as u8, *n)).collect();
    ops.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let _ = writeln!(out, "opcodes (top 40):");
    for (byte, n) in ops.iter().take(40) {
        let name = crate::vm::opcode::Opcode::decode(*byte).as_str();
        let _ = writeln!(out, "  {name:<28} {n:>12} {:>6.2}%", *n as f64 * 100.0 / s.total.max(1) as f64);
    }
    let _ = writeln!(out, "ic misses (an access that missed its inline cache; hits = executions - misses):");
    let mut sites: Vec<(&(Site, bool), &u64)> = s.misses.iter().collect();
    sites.sort();
    for ((site, cacheable), n) in sites {
        let _ = writeln!(
            out,
            "  {:<20} {:>10} {}",
            site.name(),
            n,
            if *cacheable { "(cacheable: cold, or the shape changed)" } else { "(not cacheable: misses every time)" }
        );
    }
    let mut names: Vec<(&(Site, &'static str, String), &u64)> = s.names.iter().collect();
    names.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let _ = writeln!(out, "ic misses by site, receiver kind and property name (top 40):");
    for ((site, kind, name), n) in names.into_iter().take(40) {
        let _ = writeln!(out, "  {:<20} {kind:<9} {name:<32} {n:>10}", site.name());
    }
    out
}

/// HIWAVE PATCH (vm-stats): how many times the opcode named `name` ran on this thread; `None`
/// when the counters are off or no opcode has that name, so a misspelt name never reads as 0. The
/// report lists only the 40 most run; this answers for any opcode (a test's count).
pub fn opcode_count(name: &str) -> Option<u64> {
    if !enabled() {
        return None;
    }
    let byte = (0..=255u8).find(|b| crate::vm::opcode::Opcode::decode(*b).as_str() == name)?;
    Some(STATS.with(|s| s.borrow().ops[byte as usize]))
}

/// The counters so far, as text; empty when `RUSTKIT_VM_STATS` is not set.
pub fn report() -> String {
    if !enabled() {
        return String::new();
    }
    STATS.with(|s| render(&s.borrow()))
}
