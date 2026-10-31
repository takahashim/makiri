# falloc

Fallible allocation for Makiri's engine. Every allocation the engine makes
goes through this crate, which does two things `std` does not:

- **It reports failure.** `Box::new`, `Vec::push` and `HashMap::insert` abort
  the process when memory runs out. Makiri is a library loaded into someone
  else's process, so an abort takes the host down. Here a failed allocation
  is an `Err`, and the engine turns it into a Ruby exception.
- **It can be made to fail.** Each allocation attempt consults one counter
  that `rake oom` arms, so the sweep can fail every allocation site in turn
  and check that each failure fails closed: a clean raise, or the same answer
  as when nothing failed.

`clippy.toml` bans the `std` methods this crate wraps, so a new call site in
the engine cannot go around it.

## API

Growth has a receiver, so it is a trait method named after the `std` method
it replaces:

| Trait | Methods |
| --- | --- |
| `Reserve` (`Vec`, `HashMap`, `HashSet`) | `falloc_reserve`, `falloc_reserve_exact` |
| `VecGrow` | `falloc_push`, `falloc_insert`, `falloc_extend`, `falloc_extend_from_slice`, `falloc_extend_from_within`, `falloc_resize`, `falloc_slot` |
| `MapInsert` (`HashMap`) | `falloc_insert` |
| `SetInsert` (`HashSet`) | `falloc_insert` |

Construction has no receiver, so it is a free function: `try_box`,
`try_vec_with_capacity`, `try_vec_filled`, `try_to_vec`, `try_concat`,
`try_to_boxed_slice`.

The methods carry a `falloc_` prefix because `Vec::try_reserve` and
`try_reserve_exact` are inherent `std` methods. A trait method of the same
name would never be called: the inherent one wins, and it never consults the
injection counter, so the sweep could not reach that allocation.

A few pieces exist for particular call sites:

- **`Slot`.** When an operation must change several tables together or not
  at all, take a slot in each first (`falloc_slot`, a fallible reserve of
  room for one element), then fill them. `Slot::push` cannot fail, and the
  slot borrows its vector until it is filled, so nothing else can take the
  room in between.
- **`Oom`, `OomResult`, `OomOption`.** These convert the crate's
  `Result<T, ()>` and `Option<T>` into a caller's error type with
  `.or_oom()?`. Each error type names its own out-of-memory variant once, in
  its `impl Oom`.
- **`grow_capacity`.** The growth arithmetic that `ceiling_buf` and the
  NodeSet's array use. It is pure, so it is proved at every element size by
  Verus (`../verus/ceiling_buf.rs`).

## Failure injection

The `alloc-inject` feature compiles in the counter (`src/inject.rs`). The
extension's `alloc-inject` feature turns it on, and `MAKIRI_ALLOC_INJECT=1`
builds the extension with that feature. Without the feature,
`allocation_should_fail` is a `const false` and the check compiles away.

The counter counts allocation attempts, not calls: a reserve consults it only
when it would actually allocate, so pushing into a vector with room costs the
sweep nothing. It is atomic, because a parse runs with the GVL released and
consults it from there.

## `unsafe`

`try_box` holds one `unsafe` block: stable Rust has no safe way to write a
fallible `Box<T>` allocation. That is why this is a crate of its own, and why
the engine above it can `forbid(unsafe_code)` at its root. The crate denies
`unsafe_code`, and `try_box` carries the only `allow`.

## Tests

```bash
cargo test -p falloc
cargo test -p falloc --features alloc-inject --test alloc_inject_map_insert
bundle exec rake miri    # these tests under Miri, with nupur's utf8_copy
```

`tests/alloc_inject_map_insert.rs` is an integration test of its own because
the counter is process-wide: arming it in the parallel unit-test binary would
fail unrelated tests. Miri runs in CI (`verify.yml`); `rake oom` is the sweep
over the whole engine (see `docs/internals/verification.md`).
