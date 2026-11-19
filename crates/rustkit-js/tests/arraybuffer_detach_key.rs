//! A host-made ArrayBuffer with a `[[ArrayBufferDetachKey]]` (the spec's way to make a
//! WebAssembly.Memory's buffer undetachable by script). Chrome for Testing 148.0.7778.216, a
//! `WebAssembly.Memory`'s buffer: `buffer.transfer()`, `transferToFixedLength()`,
//! `structuredClone(buffer, { transfer: [buffer] })` and `postMessage(.., [buffer])` all throw a
//! TypeError and leave the buffer attached with its length; `memory.grow()` detaches it (length 0)
//! and `memory.buffer` is then a new object; a plain ArrayBuffer's `transfer()` still works.

use boa_engine::{
    Context, JsValue, Source, js_string,
    object::builtins::JsArrayBuffer,
};

fn eval(context: &mut Context, src: &str) -> String {
    context
        .eval(Source::from_bytes(src))
        .unwrap()
        .to_string(context)
        .unwrap()
        .to_std_string_escaped()
}

#[test]
fn a_buffer_with_a_detach_key_cannot_be_transferred_by_script() {
    let mut context = Context::default();
    let key = JsValue::from(js_string!("WebAssembly.Memory"));
    // A block of storage to hand over: take one back from an ordinary buffer.
    let seed = JsArrayBuffer::new(8, &mut context).unwrap();
    let mut block = seed.detach(&JsValue::undefined()).unwrap();
    block.as_mut_slice().fill(7);
    let buffer =
        JsArrayBuffer::from_byte_block_with_detach_key(block, key.clone(), &mut context).unwrap();
    context
        .global_object()
        .set(js_string!("keyed"), buffer.clone(), true, &mut context)
        .unwrap();

    assert_eq!(
        eval(&mut context, "try { keyed.transfer(); 'transferred' } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(
        eval(&mut context, "try { keyed.transferToFixedLength(); 'transferred' } catch (e) { e.name }"),
        "TypeError"
    );
    assert_eq!(eval(&mut context, "keyed.byteLength + ',' + keyed.detached"), "8,false");

    // The host can detach it only with the key, and gets the storage back.
    assert!(buffer.detach(&JsValue::undefined()).is_err());
    assert_eq!(buffer.byte_length(), 8);
    let back = buffer.detach(&key).unwrap();
    assert_eq!(back.as_slice(), &[7u8; 8]);
    assert_eq!(eval(&mut context, "keyed.byteLength + ',' + keyed.detached"), "0,true");
}

#[test]
fn a_buffer_made_without_a_detach_key_transfers_as_before() {
    let mut context = Context::default();
    assert_eq!(
        eval(
            &mut context,
            "var plain = new ArrayBuffer(8); var moved = plain.transfer(); moved.byteLength + ',' + plain.detached"
        ),
        "8,true"
    );
}
