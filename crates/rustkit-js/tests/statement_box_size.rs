//! The rare large statements are boxed so every statement does not pay for them
//! (`statement-box`, see third_party/boa_engine/PATCHES.md).
//!
//! `Statement` was 464 bytes because `ForInLoop` and `ForOfLoop` (456) were stored inline, and every
//! statement in a script is a `Box<Statement>`: YouTube's 10.9 MB kevlar_base.js has 199k of them
//! (88 MB) and about 1,100 for-in, for-of and switch statements.
use boa_engine::ast::scope::Scope;
use boa_engine::ast::Statement;
use boa_engine::interner::{Interner, ToInternedString};
use boa_engine::parser::{Parser, Source};
use boa_engine::{Context, Source as EngineSource};

#[test]
fn statement_is_not_sized_by_its_rare_variants() {
    let size = std::mem::size_of::<Statement>();
    assert!(
        size <= 216,
        "Statement is {size} bytes: a large variant is stored inline"
    );
}

#[test]
fn boxed_statements_parse_print_and_run_as_before() {
    let src = "\
var log = [];
for (var k in { a: 1, b: 2 }) log.push(k);
for (const v of [3, 4]) log.push(v);
outer: for (const v of [5, 6]) { for (var j in { x: 1 }) { if (v === 5) continue outer; log.push(v + j); } }
switch (log.length) { case 5: log.push('five'); break; default: log.push('other'); }
log.join(',');
";
    let mut interner = Interner::default();
    let script = Parser::new(Source::from_bytes(src))
        .parse_script(&Scope::new_global(), &mut interner)
        .expect("parses");
    let printed = script.to_interned_string(&interner);
    for part in [
        "for (var k in",
        "for (const v of [3, 4])",
        "outer: for (const v of",
        "switch (log.length)",
    ] {
        assert!(
            printed.contains(part),
            "printed tree lacks {part:?}:\n{printed}"
        );
    }

    let mut context = Context::default();
    let value = context.eval(EngineSource::from_bytes(src)).expect("runs");
    let text = value
        .to_string(&mut context)
        .expect("a string")
        .to_std_string_escaped();
    assert_eq!(text, "a,b,3,4,6x,five");
}
