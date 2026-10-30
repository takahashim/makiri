# makiri_lints

Makiri's own lint over its Ruby boundary, checked by type:

- `ruby_value_on_heap`: a Ruby value (a `bridge::rb::ReprValue` type) kept
  where Ruby's GC cannot see it - in a heap container (`Vec`, `Box`,
  `HashMap`, ...), in a `static` or `thread_local!`, or directly in the data
  of a wrapped object (a `bridge::typed::Payload` type) rather than in one of
  its slots - whether a binding, a parameter, a return value, a field or a
  temporary no binding names (`f(vec![v])`, a `Box::new(move || v)`) holds
  it; a closure holds what it captures by value. Not seen: a value behind
  `dyn Trait` once coerced, a generic body for the type it is called with,
  a raw `VALUE` (an integer to the lint), and - deliberately - a value
  behind a reference or a raw pointer, or in `bridge::rb::Opaque` (a
  registered root).

A second lint, `converted_argument`, checked that no registered method took a
type magnus would convert with the caller's Ruby. Since `bridge::rb` replaced
magnus, its `TryConvert` exists only for `Value`, `bool` and Makiri's receiver
types, so such a method does not compile, and the lint is gone.

`src/value_on_heap.rs` says exactly what it reports. Run it with
`bundle exec rake dylint` from the repository root. It needs the nightly
`rust-toolchain.toml` pins (with `rustc-dev`) and
`cargo install cargo-dylint dylint-link`. The task also runs the self-test,
`src/lint_probe.rs` in the extension crate.
