# nupur_lints

The lints over the Ruby binding layer `nupur` and an extension built on it,
checked on the compiled code by resolved path and by type. They are the
layer's guarantees, which no type can state:

`raising_call_outside_protect`, `unraising_call_in_raw`,
`unguarded_no_unwind_site`, `ffi_without_gvl_proof`,
`gvl_token_outside_entry`, `forgotten_handle`, `discarded_ruby_error`,
`ruby_value_on_heap`, `c_integer_outside_int`, `runs_callers_ruby`,
`ruby_in_gvl_released_body`, `ruby_in_ruby_free_module`.

They name only `nupur::`, `nupur_sys::` and rb-sys paths. What they know of
an extension is its crate name - the crate being linted - and what the
`[nupur_lints]` table of its workspace's `dylint.toml` adds
(`src/paths.rs`, `Config`):

```toml
[nupur_lints]
extension = "my_ext"                  # when other crates are linted too
ruby_free = ["my_ext_core"]           # its crates that depend on no Ruby
no_unwind_guards = ["my_ext_core::no_unwind::must_not_unwind"]
```

Makiri's is `ext/makiri/rust/dylint.toml`. An extension's own policy is
its own lints'; Makiri's are `makiri_lints`, beside this crate.

Where rb-sys and a raw `VALUE` may be named is held by the crates'
dependencies, not a lint: only `nupur_sys` depends on rb-sys, and it
re-exports no `VALUE`.

`src/lib.rs` states each lint and the module that holds it; that module says
exactly what it reports and what it cannot see.

The layer's functions the lints know by name - those that run the caller's
Ruby, those that run a closure with the GVL released, those that lend a
String's bytes - are three tables in `src/paths.rs`, by the path each is
defined at. A function the layer adds that belongs in one must be added
there, with a probe line; `rake dylint` fails unless the self-test reports
every entry, so one that stops matching cannot go quiet. Inside the layer,
the wrappers over those functions `expect` the lint rather than `allow` it,
and the runs deny an unfulfilled expectation: a wrapper whose call stopped
being reported fails the task too.

Run them with `bundle exec rake dylint` from the repository root, which
loads `makiri_lints` with them. It needs the nightly
`../rust-toolchain.toml` pins (with `rustc-dev`) and
`cargo install cargo-dylint dylint-link`. The task then runs the self-test:
`src/lint_probe.rs` in the extension crate, `nupur/src/lint_probe.rs` and
`nupur/sys/src/lint_probe.rs` in the layer, each line of which ending in
`//~` must be reported by exactly the lints named after it.

Another extension runs them over itself from its own workspace:

```bash
cargo dylint --path <makiri>/ext/makiri/rust/lints --pattern nupur_lints --no-deps
```
