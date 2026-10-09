//! `crypto.getRandomValues` and `crypto.randomUUID` for page script.
//!
//! The bytes come from the OS random source (`/dev/urandom` on macOS and
//! Linux, `RtlGenRandom` on Windows) through one host function that answers with hex;
//! `web_crypto.js` is the object layer over it. No new dependency: nothing
//! in this crate's tree already reaches the OS RNG.

use rustkit_js::{JsError, JsRuntime, JsValue};
#[cfg(not(windows))]
use std::io::Read;

/// Web Crypto §10.1.1: at most 65536 bytes per call.
const MAX_BYTES: usize = 65536;

/// `n` bytes from the OS random source, or `None` when it cannot be read.
#[cfg(not(windows))]
fn random_bytes(n: usize) -> Option<Vec<u8>> {
    let mut bytes = vec![0u8; n];
    std::fs::File::open("/dev/urandom")
        .ok()?
        .read_exact(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// Windows has no `/dev/urandom`: it fills from `RtlGenRandom`
/// (`SystemFunction036` in advapi32), the CSPRNG the CRT and Rust's own
/// `HashMap` seeding use. Without this, `crypto.getRandomValues` and
/// `crypto.randomUUID` threw `OperationError` on every Windows page.
#[cfg(windows)]
fn random_bytes(n: usize) -> Option<Vec<u8>> {
    #[link(name = "advapi32")]
    extern "system" {
        #[link_name = "SystemFunction036"]
        fn rtl_gen_random(buffer: *mut u8, length: u32) -> u8;
    }
    let mut bytes = vec![0u8; n];
    // The call takes a u32 length; the caller caps n at 65536.
    let length = u32::try_from(n).ok()?;
    if length == 0 {
        return Some(bytes);
    }
    // SAFETY: `bytes` is a live allocation of exactly `length` bytes.
    let ok = unsafe { rtl_gen_random(bytes.as_mut_ptr(), length) };
    (ok != 0).then_some(bytes)
}

pub(crate) fn install(runtime: &mut JsRuntime) -> Result<(), JsError> {
    runtime.register_host_function(
        "__rustkit_random_bytes",
        1,
        Box::new(|args| {
            let n = match args.first() {
                Some(JsValue::Number(n)) if *n >= 0.0 && *n <= MAX_BYTES as f64 => *n as usize,
                _ => return JsValue::Null,
            };
            match random_bytes(n) {
                Some(bytes) => JsValue::String(bytes.iter().map(|b| format!("{b:02x}")).collect()),
                None => JsValue::Null,
            }
        }),
    )?;
    runtime
        .evaluate_script(include_str!("web_crypto.js"))
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_os_source_answers_with_the_requested_length() {
        assert_eq!(random_bytes(0).unwrap().len(), 0);
        let a = random_bytes(32).unwrap();
        assert_eq!(a.len(), 32);
        assert_ne!(a, random_bytes(32).unwrap());
    }
}
