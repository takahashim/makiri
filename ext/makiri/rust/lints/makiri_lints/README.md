# makiri_lints

The lints over Makiri's Ruby binding layer (`nupur`) and the extension built
on it, checked on the compiled code by resolved path and by type, in two
groups:

- **The layer's guarantees** (they name only `nupur::` and rb-sys paths):
  `raising_call_outside_protect`, `unraising_call_in_raw`,
  `unguarded_no_unwind_site`, `ffi_without_gvl_proof`,
  `gvl_token_outside_entry`, `forgotten_handle`, `discarded_ruby_error`,
  `ruby_value_on_heap`, `c_integer_outside_int`, `runs_callers_ruby`,
  `ruby_in_gvl_released_body`, and the layer's half of
  `raw_ruby_outside_bridge`.
- **The extension's own layout**: `ruby_layer_in_engine`,
  `infallible_allocation`, `raw_node_crossing`, `ruby_surface_in_bridge`,
  and the extension's half of `raw_ruby_outside_bridge`.

`src/lib.rs` states each lint and the module that holds it; that module says
exactly what it reports and what it cannot see.

Run them with `bundle exec rake dylint` from the repository root. It needs
the nightly `rust-toolchain.toml` pins (with `rustc-dev`) and
`cargo install cargo-dylint dylint-link`. The task then runs the self-test:
`src/lint_probe.rs` in the extension crate and `nupur/src/lint_probe.rs` in
the layer, each line of which ending in `//~` must be reported by exactly the
lints named after it.
