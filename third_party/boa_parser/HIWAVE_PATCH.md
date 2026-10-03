# Vendored boa_parser 0.20.0, with one backported fix

Source: crates.io `boa_parser` 0.20.0, unmodified except the lines marked
`HIWAVE PATCH` in `src/parser/statement/declaration/lexical.rs` (one changed
line and one comment line). Wired in through `[patch.crates-io]` in the
workspace `Cargo.toml`, next to `boa_gc`. License as upstream (see
`Cargo.toml`).

**Bug.** `allowed_token_after_let` decides whether a statement that starts
with `let` is a declaration by looking at the next token. `of` is lexed as
`Keyword::Of`, which the list did not contain, so `let of = 1;` was parsed as
the expression `let` followed by a stray `of` ("expected token ';', got 'of'
in expression statement"), and `for (let of = 0; ...)` failed too. `of` is an
ordinary binding name: minified React bundles use it. github.com's
`react-core.js` (line 4, col 82960) and `landing-pages.js` (col 17741) are two
files that died on it, which stopped `behaviors.js` (it imports `react`) and
therefore the page's start-up. `var of`, `const of` and `let get/set/from/as`
already worked.

**Fix.** Add `Keyword::Of` to the list. This is upstream boa-dev/boa#4593
("Allow 'of' as variable name in `let` declarations", merged 2026-02-03, merge
commit `029248e4c4bc229513d03abd816f4e044b51cd80`), present in `boa_parser`
0.22.0 and not in 0.20.0 or 0.21.1.

**Check that the diff is what this says.**

```
diff -r ~/.cargo/registry/src/*/boa_parser-0.20.0/src third_party/boa_parser/src
```

prints exactly the two lines at `lexical.rs:133-134` (the comment and the
list entry). `.cargo_vcs_info.json` and `Cargo.toml.orig` are not vendored.

**Tests.** `crates/rustkit-js/tests/let_of.rs`: red on stock 0.20.0 (three of
its four tests), green with this patch.

**Removing it.** Upgrade `boa_engine` to 0.22 (or later) and delete this
directory, the `[patch.crates-io]` line and the `exclude` entry. That is a
cross-cutting change (two major versions) and was not done here.
