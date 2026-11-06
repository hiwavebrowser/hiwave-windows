//! HIWAVE PATCH: the host functions on the native re-entry stack.
//!
//! `JsObject::call` / `construct` nest `Context::run` and count that in
//! `host_call_depth`. The count has no names. While armed (the console log),
//! each re-entry records the function it enters. The first time a climb
//! crosses 256, 512, and 1000, the hook receives a histogram of those names
//! and the innermost 20 in order. A null hook or a disarmed flag does nothing
//! on the hot path.

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

use crate::{
    Context, JsObject,
    builtins::function::{BoundFunction, OrdinaryFunction},
    native_function::NativeFunctionObject,
};

/// `depth` is the host re-entry count. `histogram` is `name=count` pairs,
/// most frequent first. `frames` is the innermost 20 names, outermost of
/// that window first.
pub type HostStackDiag = fn(depth: usize, histogram: &str, frames: &str);

static HOOK: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static ARMED: AtomicBool = AtomicBool::new(false);
/// The first mark. 512 and 1000 stay put. Tests lower this: a debug build
/// overflows the native stack before a real climb of 256.
static FIRST_MARK: AtomicUsize = AtomicUsize::new(256);

const NAME_CHARS: usize = 80;
const HISTOGRAM_NAMES: usize = 40;
const FRAME_WINDOW: usize = 20;

/// Install the log hook. `None` clears it.
pub fn set_host_stack_diag(hook: Option<HostStackDiag>) {
    let ptr = match hook {
        Some(hook) => hook as *mut (),
        None => std::ptr::null_mut(),
    };
    HOOK.store(ptr, Ordering::Relaxed);
}

/// Record names only while this is set. The console log turns it on.
pub fn set_host_stack_armed(on: bool) {
    ARMED.store(on, Ordering::Relaxed);
}

/// Move the first log mark. `None` restores 256. 512 and 1000 are unchanged.
pub fn set_host_stack_first_mark(mark: Option<usize>) {
    FIRST_MARK.store(mark.unwrap_or(256), Ordering::Relaxed);
}

pub(crate) fn enter(context: &mut Context, func: &JsObject) -> bool {
    if !ARMED.load(Ordering::Relaxed) {
        return false;
    }
    context.vm.host_frames.push(function_name(func));
    note(context);
    true
}

pub(crate) fn leave(context: &mut Context, pushed: bool) {
    if !pushed {
        return;
    }
    context.vm.host_frames.pop();
    if context.vm.host_call_depth <= 1 {
        context.vm.host_stack_logged = 0;
    }
}

fn note(context: &mut Context) {
    let depth = context.vm.host_call_depth;
    let bit = if depth == FIRST_MARK.load(Ordering::Relaxed) {
        1u8
    } else if depth == 512 {
        2
    } else if depth == 1000 {
        4
    } else {
        return;
    };
    if context.vm.host_stack_logged & bit != 0 {
        return;
    }
    context.vm.host_stack_logged |= bit;
    let ptr = HOOK.load(Ordering::Relaxed);
    if ptr.is_null() {
        return;
    }
    let (histogram, frames) = snapshot(&context.vm.host_frames);
    // SAFETY: `ptr` was stored from a `HostStackDiag`, or it is null and we
    // already returned. The engine installs one static function.
    let hook: HostStackDiag = unsafe { std::mem::transmute(ptr) };
    hook(context.vm.host_call_depth, &histogram, &frames);
}

fn snapshot(stack: &[String]) -> (String, String) {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for name in stack {
        if let Some(slot) = counts.iter_mut().find(|(n, _)| *n == name.as_str()) {
            slot.1 += 1;
        } else {
            counts.push((name.as_str(), 1));
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let mut histogram = String::new();
    for (i, (name, count)) in counts.iter().take(HISTOGRAM_NAMES).enumerate() {
        if i > 0 {
            histogram.push(',');
        }
        histogram.push_str(&flat(name));
        histogram.push('=');
        histogram.push_str(&count.to_string());
    }
    if counts.len() > HISTOGRAM_NAMES {
        histogram.push_str(",+");
        histogram.push_str(&(counts.len() - HISTOGRAM_NAMES).to_string());
    }
    let start = stack.len().saturating_sub(FRAME_WINDOW);
    let mut frames = String::new();
    if start > 0 {
        frames.push_str("… > ");
    }
    for (i, name) in stack[start..].iter().enumerate() {
        if i > 0 {
            frames.push_str(" > ");
        }
        frames.push_str(&flat(name));
    }
    (histogram, frames)
}

fn function_name(func: &JsObject) -> String {
    if let Some(fun) = func.downcast_ref::<OrdinaryFunction>() {
        return named(&fun.code.name().to_std_string_escaped());
    }
    if let Some(native) = func.downcast_ref::<NativeFunctionObject>() {
        return named(&native.name.to_std_string_escaped());
    }
    if let Some(bound) = func.downcast_ref::<BoundFunction>() {
        return named(&format!("bound {}", function_name(bound.target_function())));
    }
    "(anonymous)".to_string()
}

fn named(name: &str) -> String {
    if name.is_empty() {
        "(anonymous)".to_string()
    } else {
        flat(name)
    }
}

fn flat(name: &str) -> String {
    let mut out: String = name
        .chars()
        .take(NAME_CHARS)
        .map(|c| match c {
            '"' | '\n' | '\r' | ',' | '=' => '\'',
            _ => c,
        })
        .collect();
    if name.chars().count() > NAME_CHARS {
        out.push('…');
    }
    out
}
