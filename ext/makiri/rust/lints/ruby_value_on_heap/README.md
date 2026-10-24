# ruby_value_on_heap

Makiri's own lint: a Ruby value (a magnus `ReprValue` type) kept where Ruby's
GC cannot see it - in a heap container (`Vec`, `Box`, `HashMap`, ...), in a
`static` or `thread_local!`, or directly in the data of a wrapped object (a
magnus `TypedData` or Makiri `Hooks` type) rather than in a field its mark
traces. The rule is magnus's ("Ruby objects must be kept on the stack"); magnus
leaves it to its users, and this checks it by type. `src/lib.rs` says what it
reports and what it leaves alone.

Run it with `bundle exec rake dylint` from the repository root. It needs the
nightly `rust-toolchain.toml` pins (with `rustc-dev`) and
`cargo install cargo-dylint dylint-link`. The task also runs the self-test,
`src/bridge/lint_probe.rs` in the extension crate.

Known gaps: a temporary passed straight to a function, never bound, is not
seen (`f(vec![v])`), and neither is a value behind a reference or a raw
pointer - deliberately, since the value itself is elsewhere.
