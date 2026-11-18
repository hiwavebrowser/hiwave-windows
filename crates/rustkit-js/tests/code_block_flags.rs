//! The interpreter's code-block flags are one bit each and no two share a bit: two patches of the
//! vendored engine (the sort shortcut's mark and a script's error-detail mark) once took the same two.

#[test]
fn no_two_code_block_flags_share_a_bit() {
    let (clashes, count) = rustkit_js::code_block_flag_clashes();
    assert!(clashes.is_empty(), "flags that are not one bit of their own: {clashes:?}");
    assert!(count >= 13, "the flags were not listed: {count}");
}
