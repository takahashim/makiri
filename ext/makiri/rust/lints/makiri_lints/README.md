# makiri_lints

Makiri's own lint over its Ruby boundary, checked by type:

- `ruby_value_on_heap`: a Ruby value (a `bridge::rb::ReprValue` type) kept
  where Ruby's GC cannot see it - in a heap container (`Vec`, `Box`,
  `HashMap`, ...), in a `static` or `thread_local!`, or directly in the data
  of a wrapped object (a `bridge::typed::Hooks` type) rather than in a field
  its mark traces. Not seen: a temporary passed straight to a call, never
  bound (`f(vec![v])`), and - deliberately - a value behind a reference or a
  raw pointer, or in `bridge::rb::Opaque` (a registered root).

A second lint, `converted_argument`, checked that no registered method took a
type magnus would convert with the caller's Ruby. Since `bridge::rb` replaced
magnus, its `TryConvert` exists only for `Value`, `bool` and Makiri's receiver
types, so such a method does not compile, and the lint is gone.

`src/value_on_heap.rs` says exactly what it reports. Run it with
`bundle exec rake dylint` from the repository root. It needs the nightly
`rust-toolchain.toml` pins (with `rustc-dev`) and
`cargo install cargo-dylint dylint-link`. The task also runs the self-test,
`src/lint_probe.rs` in the extension crate.
