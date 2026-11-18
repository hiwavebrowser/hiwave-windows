//! Byte work the embedder's scripts hand to the host: response bodies,
//! UTF-8, base64 and binary strings.
//!
//! These are natives on the host object (`__rk`, see
//! [`JsRuntime::evaluate_with_host`](crate::JsRuntime::evaluate_with_host)),
//! so only the embedder's own scripts can call them. Each does in one call
//! what the scripts used to do with a loop turn per byte and a string built
//! by `+=` (copied on every step): a 256 KB response took 3 s to reach the
//! page and 1 MB did not arrive inside the script budget.
//!
//! - `takeBytes(key)`: the bytes [`JsRuntime::stash_bytes`](crate::JsRuntime::stash_bytes)
//!   left under `key`, as an `ArrayBuffer`, once; `undefined` after that.
//! - `utf8Decode(bytes, flush)`: `[text, bytesUsed, sawError]`. Malformed
//!   input becomes U+FFFD, one per maximal subpart (the Encoding Standard's
//!   and `std`'s rule). Without `flush`, an incomplete sequence at the end
//!   is left unused for the next call.
//! - `utf8Encode(string)`: a `Uint8Array`; a lone surrogate is U+FFFD.
//! - `binaryDecode(bytes)`: the string with one code unit per byte.
//! - `binaryEncode(string)`: its code units as bytes, or `null` when one is
//!   above 255.
//! - `base64Decode(string)`: a `Uint8Array`, or `null` when `string` is not
//!   base64 (standard alphabet, padding optional, trailing bits dropped).
//! - `base64Encode(bytes)`: padded standard base64.
//!
//! `bytes` is an `ArrayBuffer` or a view on one.
//!
//! Canvas pixels cross the same way, through one slot the embedder holds
//! ([`PixelSlot`]):
//!
//! - `pixelsOut(length, fresh)`: a new `Uint8ClampedArray` of `length`
//!   bytes, the realm's own intrinsic whatever the page did to its
//!   globals, holding what the embedder left in the slot (zeros with
//!   `fresh`, or when it left nothing of that length). The slot is emptied
//!   either way.
//! - `pixelsIn(array, keep)`: `array`'s length in bytes when it is a
//!   `Uint8ClampedArray` on a live `ArrayBuffer`, `-1` when its buffer is
//!   detached, `-2` for anything else. With `keep`, a copy of its bytes is
//!   left in the slot for the embedder. No property of `array` is read.

use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine as _;
use boa_engine::object::builtins::{JsArray, JsArrayBuffer, JsTypedArray, JsUint8Array, JsUint8ClampedArray};
use boa_engine::object::ObjectInitializer;
use boa_engine::string::{JsStr, JsStrVariant};
use boa_engine::{js_string, Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// One run of canvas pixels on its way between the embedder and script
/// (`__rk.pixelsOut`, `__rk.pixelsIn`): whoever fills it has it emptied by
/// the very next step of the same call, so one slot serves a runtime.
#[derive(Clone, Default)]
pub struct PixelSlot(Rc<RefCell<Option<Vec<u8>>>>);

impl PixelSlot {
    /// Leave `bytes` for the script's next `pixelsOut`.
    pub fn put(&self, bytes: Vec<u8>) {
        *self.0.borrow_mut() = Some(bytes);
    }

    /// What the script's last `pixelsIn(array, true)` left, once.
    pub fn take(&self) -> Option<Vec<u8>> {
        self.0.borrow_mut().take()
    }
}

/// The most `pixelsOut` makes in one array: the bytes of the largest
/// bitmap a canvas keeps (16384 x 16384, RGBA).
const MAX_PIXEL_BYTES: usize = 16384 * 16384 * 4;

/// Bytes waiting for script to take them, by key.
pub(crate) type Stash = Rc<RefCell<HashMap<u64, Vec<u8>>>>;

/// What stands behind the page's blob URLs: a copy of the Blob's bytes and
/// its type, by URL (without fragment). Kept on the host side: the engine
/// reads it without running any script (`JsRuntime::blob_bytes`), and the
/// page has no object to replace. One store per runtime, so the documents
/// of a frame tree share it; each record belongs to the origin of the
/// document that made it, and only that origin's documents reach it.
pub(crate) type Blobs = Rc<RefCell<BlobStore>>;

/// The most the blob URLs of one runtime keep alive, in bytes. Every
/// `createObjectURL` copies its Blob out of the script heap, so without a
/// bound a loop over one large Blob grows the process without limit. Past
/// it a new URL has nothing behind it (as for a revoked one).
pub(crate) const MAX_BLOB_STORE_BYTES: usize = 256 * 1024 * 1024;

pub(crate) struct BlobRecord {
    pub(crate) bytes: Vec<u8>,
    pub(crate) kind: String,
    /// The realm of the document that made it.
    realm: usize,
    /// That document's origin when the URL was made (`null`: opaque).
    origin: String,
}

#[derive(Default)]
pub(crate) struct BlobStore {
    records: HashMap<String, BlobRecord>,
    /// The bytes of all records together.
    held: usize,
    /// The bound on `held`, when not [`MAX_BLOB_STORE_BYTES`].
    pub(crate) limit: Option<usize>,
    /// Each realm's origin, as the embedder gave it
    /// (`JsRuntime::set_realm_origin`): never what the page says it is.
    origins: HashMap<usize, String>,
    /// Where the names' random bytes come from (`JsRuntime::set_blob_random`).
    pub(crate) random: Option<fn(usize) -> Option<Vec<u8>>>,
    /// Numbers the URLs made while there is no random source; nothing is
    /// kept behind those.
    last: u64,
}

impl BlobStore {
    pub(crate) fn set_origin(&mut self, realm: usize, origin: &str) {
        self.origins.insert(realm, origin.to_string());
    }

    fn origin_of(&self, realm: usize) -> &str {
        self.origins.get(&realm).map_or("null", String::as_str)
    }

    /// Whether a document in `realm` may read or revoke `record`: it made
    /// it, or it is of the origin that did. An opaque origin is only itself.
    fn reaches(&self, realm: usize, record: &BlobRecord) -> bool {
        record.realm == realm || (record.origin != "null" && record.origin == self.origin_of(realm))
    }

    /// A new blob URL for a document in `realm` (`blob:<origin>/<uuid>`,
    /// File API §8.3.1), with `blob` (bytes and type) behind it, and whether
    /// what was asked was done: `false` when the bytes were not kept (the
    /// store is full, or there is no random source to name them with).
    fn make(&mut self, realm: usize, blob: Option<(Vec<u8>, String)>) -> (String, bool) {
        let origin = self.origin_of(realm).to_string();
        let Some(name) = self
            .random
            .and_then(|random| random(16))
            .filter(|b| b.len() == 16)
            .map(|b| uuid(&b))
        else {
            self.last += 1;
            return (format!("blob:{origin}/{}", self.last), blob.is_none());
        };
        let url = format!("blob:{origin}/{name}");
        let Some((bytes, kind)) = blob else {
            return (url, true);
        };
        // A name is never reused: a record another document made is not
        // replaced, whatever the random source says.
        if self.records.contains_key(&url)
            || bytes.len()
                > self
                    .limit
                    .unwrap_or(MAX_BLOB_STORE_BYTES)
                    .saturating_sub(self.held)
        {
            return (url, false);
        }
        self.held += bytes.len();
        self.records.insert(
            url.clone(),
            BlobRecord {
                bytes,
                kind,
                realm,
                origin,
            },
        );
        (url, true)
    }

    pub(crate) fn get(&self, realm: usize, url: &str) -> Option<&BlobRecord> {
        self.records
            .get(url)
            .filter(|record| self.reaches(realm, record))
    }

    fn revoke(&mut self, realm: usize, url: &str) {
        if self.get(realm, url).is_some() {
            if let Some(record) = self.records.remove(url) {
                self.held -= record.bytes.len();
            }
        }
    }

    /// A realm is gone: so are the URLs its document made (File API §8.3.5,
    /// unloading a document revokes its blob URLs).
    pub(crate) fn drop_realm(&mut self, realm: usize) {
        self.records.retain(|_, record| record.realm != realm);
        self.held = self.records.values().map(|record| record.bytes.len()).sum();
        self.origins.remove(&realm);
    }
}

/// RFC 9562 version 4 from 16 random bytes.
fn uuid(random: &[u8]) -> String {
    let mut b = [0u8; 16];
    b.copy_from_slice(random);
    b[6] = (b[6] & 0x0F) | 0x40;
    b[8] = (b[8] & 0x3F) | 0x80;
    let hex = |range: std::ops::Range<usize>| {
        b[range]
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect::<String>()
    };
    format!(
        "{}-{}-{}-{}-{}",
        hex(0..4),
        hex(4..6),
        hex(6..8),
        hex(8..10),
        hex(10..16)
    )
}

/// The realm a blob native is called for: the first argument, which only
/// the embedder's own scripts can supply (`__realm`).
fn realm_arg(args: &[JsValue]) -> JsResult<usize> {
    args.first()
        .and_then(JsValue::as_number)
        .filter(|n| *n >= 0.0)
        .map(|n| n as usize)
        .ok_or_else(|| JsNativeError::typ().with_message("expected a realm").into())
}

const BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// Standard base64 (padding optional) as bytes; `None` when it is not.
pub(crate) fn base64_decode(ascii: &[u8]) -> Option<Vec<u8>> {
    BASE64.decode(ascii).ok()
}

/// Run `f` over the bytes of `value`: an `ArrayBuffer`, or the part of one
/// a typed array or `DataView` covers.
fn with_bytes<T>(
    value: Option<&JsValue>,
    context: &mut Context,
    f: impl FnOnce(&[u8]) -> T,
) -> JsResult<T> {
    let not_bytes =
        || JsNativeError::typ().with_message("expected an ArrayBuffer or a view on one");
    let object = value.and_then(JsValue::as_object).ok_or_else(not_bytes)?;
    let (buffer, start, length) = match JsArrayBuffer::from_object(object.clone()) {
        Ok(buffer) => {
            let length = buffer.byte_length();
            (buffer, 0, length)
        }
        Err(_) => {
            let view = JsTypedArray::from_object(object.clone()).map_err(|_| not_bytes())?;
            let buffer = view.buffer(context)?;
            let buffer = buffer.as_object().ok_or_else(not_bytes)?;
            let buffer = JsArrayBuffer::from_object(buffer.clone()).map_err(|_| not_bytes())?;
            (
                buffer,
                view.byte_offset(context)?,
                view.byte_length(context)?,
            )
        }
    };
    let data = buffer
        .data()
        .ok_or_else(|| JsNativeError::typ().with_message("the ArrayBuffer is detached"))?;
    let end = start.saturating_add(length).min(data.len());
    Ok(f(&data[start.min(end)..end]))
}

fn array_buffer(bytes: &[u8], context: &mut Context) -> JsResult<JsArrayBuffer> {
    let buffer = JsArrayBuffer::new(bytes.len(), context)?;
    if let Some(mut data) = buffer.data_mut() {
        data.copy_from_slice(bytes);
    }
    Ok(buffer)
}

fn uint8_array(bytes: &[u8], context: &mut Context) -> JsResult<JsValue> {
    let buffer = array_buffer(bytes, context)?;
    Ok(JsUint8Array::from_array_buffer(buffer, context)?.into())
}

/// Lossy UTF-8: `(text, bytes used, whether anything was malformed)`.
pub(crate) fn utf8_decode(bytes: &[u8], flush: bool) -> (String, usize, bool) {
    let mut text = String::with_capacity(bytes.len());
    let (mut at, mut bad) = (0, false);
    while at < bytes.len() {
        match std::str::from_utf8(&bytes[at..]) {
            Ok(rest) => {
                text.push_str(rest);
                at = bytes.len();
            }
            Err(error) => {
                let valid = error.valid_up_to();
                // The prefix is valid by `valid_up_to`'s contract.
                text.push_str(std::str::from_utf8(&bytes[at..at + valid]).unwrap_or_default());
                at += valid;
                match error.error_len() {
                    Some(len) => {
                        text.push('\u{FFFD}');
                        bad = true;
                        at += len;
                    }
                    // An incomplete sequence at the end of the input.
                    None if flush => {
                        text.push('\u{FFFD}');
                        bad = true;
                        at = bytes.len();
                    }
                    None => break,
                }
            }
        }
    }
    (text, at, bad)
}

/// UTF-8 of a string's code units; a lone surrogate is U+FFFD.
pub(crate) fn utf8_encode(string: JsStr<'_>) -> Vec<u8> {
    match string.variant() {
        JsStrVariant::Latin1(units) if units.is_ascii() => units.to_vec(),
        JsStrVariant::Latin1(units) => units
            .iter()
            .map(|&u| u as char)
            .collect::<String>()
            .into_bytes(),
        JsStrVariant::Utf16(units) => char::decode_utf16(units.iter().copied())
            .map(|c| c.unwrap_or('\u{FFFD}'))
            .collect::<String>()
            .into_bytes(),
    }
}

fn string_arg(args: &[JsValue]) -> JsResult<JsString> {
    args.first().and_then(JsValue::as_string).ok_or_else(|| {
        JsNativeError::typ()
            .with_message("expected a string")
            .into()
    })
}

/// Add the byte natives to the host object under construction.
pub(crate) fn install(host: &mut ObjectInitializer<'_>, stash: &Stash, blobs: &Blobs, pixels: &PixelSlot) {
    let make = blobs.clone();
    // SAFETY: the closures below hold an `Rc` of plain bytes and strings:
    // nothing the collector would need to trace.
    // `blobUrl(realm, bytes?, type?)`: a new blob URL for the document in
    // `realm`, with a copy of `bytes` behind it when given (a real
    // ArrayBuffer or view, `with_bytes`): `[url, done]`.
    let blob_url = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let realm = realm_arg(args)?;
            let blob = match args.get(1).filter(|v| !v.is_undefined()) {
                Some(bytes) => {
                    let bytes = with_bytes(Some(bytes), context, <[u8]>::to_vec)?;
                    let kind = args
                        .get(2)
                        .and_then(JsValue::as_string)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default();
                    Some((bytes, kind))
                }
                None => None,
            };
            let (url, done) = make.borrow_mut().make(realm, blob);
            let parts = [
                JsValue::from(JsString::from(url.as_str())),
                JsValue::from(done),
            ];
            Ok(JsArray::from_iter(parts, context).into())
        })
    };
    let get = blobs.clone();
    // `blobGet(realm, url)`: `[ArrayBuffer, type]`, or `undefined` when
    // nothing a document in `realm` may read stands behind `url`.
    let blob_get = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let realm = realm_arg(args)?;
            let url = string_arg(&args[1.min(args.len())..])?.to_std_string_escaped();
            let found = get
                .borrow()
                .get(realm, &url)
                .map(|record| (record.bytes.clone(), record.kind.clone()));
            match found {
                Some((bytes, kind)) => {
                    let buffer = array_buffer(&bytes, context)?;
                    let parts = [
                        JsValue::from(buffer),
                        JsValue::from(JsString::from(kind.as_str())),
                    ];
                    Ok(JsArray::from_iter(parts, context).into())
                }
                None => Ok(JsValue::undefined()),
            }
        })
    };
    let revoke = blobs.clone();
    // `blobRevoke(realm, url)`.
    let blob_revoke = unsafe {
        NativeFunction::from_closure(move |_this, args, _context| {
            if let (Ok(realm), Ok(url)) = (realm_arg(args), string_arg(&args[1.min(args.len())..]))
            {
                revoke
                    .borrow_mut()
                    .revoke(realm, &url.to_std_string_escaped());
            }
            Ok(JsValue::undefined())
        })
    };
    host.function(blob_url, js_string!("blobUrl"), 3)
        .function(blob_get, js_string!("blobGet"), 2)
        .function(blob_revoke, js_string!("blobRevoke"), 2);
    let taken = stash.clone();
    // SAFETY: the closure holds an `Rc` of plain bytes: nothing the
    // collector would need to trace.
    let take_bytes = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let key = args.first().and_then(JsValue::as_number).unwrap_or(-1.0);
            if key < 0.0 || key.fract() != 0.0 {
                return Ok(JsValue::undefined());
            }
            let bytes = taken.borrow_mut().remove(&(key as u64));
            match bytes {
                Some(bytes) => Ok(array_buffer(&bytes, context)?.into()),
                None => Ok(JsValue::undefined()),
            }
        })
    };
    let decode = NativeFunction::from_fn_ptr(|_this, args, context| {
        let flush = args.get(1).is_some_and(JsValue::to_boolean);
        let (text, used, bad) =
            with_bytes(args.first(), context, |bytes| utf8_decode(bytes, flush))?;
        let parts = [
            JsValue::from(JsString::from(text.as_str())),
            JsValue::from(used as f64),
            JsValue::from(bad),
        ];
        Ok(JsArray::from_iter(parts, context).into())
    });
    let encode = NativeFunction::from_fn_ptr(|_this, args, context| {
        let bytes = utf8_encode(string_arg(args)?.as_str());
        uint8_array(&bytes, context)
    });
    let binary_decode = NativeFunction::from_fn_ptr(|_this, args, context| {
        let text = with_bytes(args.first(), context, |bytes| {
            JsString::from(JsStr::latin1(bytes))
        })?;
        Ok(text.into())
    });
    let binary_encode = NativeFunction::from_fn_ptr(|_this, args, context| {
        let string = string_arg(args)?;
        let bytes: Option<Vec<u8>> = match string.as_str().variant() {
            JsStrVariant::Latin1(units) => Some(units.to_vec()),
            JsStrVariant::Utf16(units) => units.iter().map(|&u| u8::try_from(u).ok()).collect(),
        };
        match bytes {
            Some(bytes) => uint8_array(&bytes, context),
            None => Ok(JsValue::null()),
        }
    });
    let base64_decode = NativeFunction::from_fn_ptr(|_this, args, context| {
        let string = string_arg(args)?;
        let ascii: Option<Vec<u8>> = match string.as_str().variant() {
            JsStrVariant::Latin1(units) => Some(units.to_vec()),
            JsStrVariant::Utf16(units) => units.iter().map(|&u| u8::try_from(u).ok()).collect(),
        };
        match ascii.and_then(|ascii| base64_decode(&ascii)) {
            Some(bytes) => uint8_array(&bytes, context),
            None => Ok(JsValue::null()),
        }
    });
    let base64_encode = NativeFunction::from_fn_ptr(|_this, args, context| {
        let text = with_bytes(args.first(), context, |bytes| BASE64.encode(bytes))?;
        Ok(JsString::from(JsStr::latin1(text.as_bytes())).into())
    });
    let slot = pixels.clone();
    // SAFETY: the closure holds an `Rc` of plain bytes, no GC handle.
    let pixels_out = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let held = slot.take();
            let length = args.first().and_then(JsValue::as_number).unwrap_or(-1.0);
            if !(length >= 0.0 && length <= MAX_PIXEL_BYTES as f64 && length.fract() == 0.0) {
                return Err(JsNativeError::range().with_message("not a pixel length").into());
            }
            let length = length as usize;
            let buffer = JsArrayBuffer::new(length, context)?;
            let fresh = args.get(1).is_some_and(JsValue::to_boolean);
            if let (Some(held), Some(mut data)) = (held.filter(|held| !fresh && held.len() == length), buffer.data_mut()) {
                data.copy_from_slice(&held);
            }
            Ok(JsUint8ClampedArray::from_array_buffer(buffer, context)?.into())
        })
    };
    let slot = pixels.clone();
    // SAFETY: as above.
    let pixels_in = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let keep = args.get(1).is_some_and(JsValue::to_boolean);
            let Some(view) = args.first().and_then(JsValue::as_object).and_then(|o| JsUint8ClampedArray::from_object(o.clone()).ok()) else {
                return Ok(JsValue::from(-2));
            };
            let Some(buffer) = view.buffer(context)?.as_object().and_then(|o| JsArrayBuffer::from_object(o.clone()).ok()) else {
                return Ok(JsValue::from(-2));
            };
            let Some(data) = buffer.data() else {
                return Ok(JsValue::from(-1));
            };
            let start = view.byte_offset(context)?;
            let end = start.saturating_add(view.byte_length(context)?).min(data.len());
            let bytes = &data[start.min(end)..end];
            if keep {
                slot.put(bytes.to_vec());
            }
            Ok(JsValue::from(bytes.len() as f64))
        })
    };
    host.function(pixels_out, js_string!("pixelsOut"), 2)
        .function(pixels_in, js_string!("pixelsIn"), 2);
    host.function(take_bytes, js_string!("takeBytes"), 1)
        .function(decode, js_string!("utf8Decode"), 2)
        .function(encode, js_string!("utf8Encode"), 1)
        .function(binary_decode, js_string!("binaryDecode"), 1)
        .function(binary_encode, js_string!("binaryEncode"), 1)
        .function(base64_decode, js_string!("base64Decode"), 1)
        .function(base64_encode, js_string!("base64Encode"), 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_utf8_is_one_replacement_per_maximal_subpart() {
        // F0 9F 98 is a truncated four-byte sequence, then 'a'; C0 can
        // start nothing; E2 82 at the end is incomplete.
        assert_eq!(
            utf8_decode(b"\xF0\x9F\x98a", true),
            ("\u{FFFD}a".to_string(), 4, true)
        );
        assert_eq!(
            utf8_decode(b"\xC0\x80", true),
            ("\u{FFFD}\u{FFFD}".to_string(), 2, true)
        );
        assert_eq!(
            utf8_decode(b"ok\xE2\x82", true),
            ("ok\u{FFFD}".to_string(), 4, true)
        );
        assert_eq!(
            utf8_decode("é€😀".as_bytes(), true),
            ("é€😀".to_string(), 9, false)
        );
    }

    #[test]
    fn an_incomplete_tail_waits_for_the_next_chunk_when_streaming() {
        assert_eq!(
            utf8_decode(b"ok\xE2\x82", false),
            ("ok".to_string(), 2, false)
        );
        assert_eq!(
            utf8_decode(b"\xE2\x82\xAC", false),
            ("€".to_string(), 3, false)
        );
    }

    #[test]
    fn a_lone_surrogate_encodes_as_the_replacement_character() {
        let units = [0x61, 0xD800, 0x62];
        assert_eq!(utf8_encode(JsStr::utf16(&units)), b"a\xEF\xBF\xBDb");
        assert_eq!(utf8_encode(JsStr::latin1(b"\xE9")), "é".as_bytes());
    }
}
