# makiri_lints

Makiri's own lints over the magnus boundary, each a rule magnus states or
implies and leaves to its users, checked by type:

- `ruby_value_on_heap`: a Ruby value (a magnus `ReprValue` type) kept where
  Ruby's GC cannot see it - in a heap container (`Vec`, `Box`, `HashMap`,
  ...), in a `static` or `thread_local!`, or directly in the data of a
  wrapped object (a magnus `TypedData` or Makiri `Hooks` type) rather than in
  a field its mark traces. Not seen: a temporary passed straight to a call,
  never bound (`f(vec![v])`), and - deliberately - a value behind a reference
  or a raw pointer.
- `converted_argument`: a function registered with `method!` / `function!`
  whose receiver or argument is a type magnus converts with the caller's
  Ruby (`RString` by `to_str`, `i64` by `to_int`, `RHash`, `Symbol`, ...).
  Accepted: `Value`, `bool`, a reference to a `TypedData` type, and the crate's own
  `TryConvert` types.

The module docs in `src/` say exactly what each reports. Run them with
`bundle exec rake dylint` from the repository root. They need the nightly
`rust-toolchain.toml` pins (with `rustc-dev`) and
`cargo install cargo-dylint dylint-link`. The task also runs the self-test,
`src/lint_probe.rs` in the extension crate.
