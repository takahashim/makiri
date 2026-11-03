# makiri_lints

Makiri's own lints, over the crates it ships - the extension, the engine,
and the binding layer under them:

- `infallible_allocation`: an allocation that aborts on OOM. Every
  allocation goes through `falloc`, and an exception message through
  `Msg`, so an OOM raises.
- `raw_node_crossing`: a borrowed HTML node handle made a lifetime-free
  `RawNode` in `bridge/`, each one allowed where it is made, with its reason.
- `ruby_surface_in_bridge`: a Ruby method defined, or an argument list
  split, in `bridge/` (or in the layer, outside `nupur::rb::class`, `raw`
  and `args`) - the glue's work.

They are Makiri's policy, not the layer's, which is why they may name its
modules (`bridge`, `glue`) and engine. The layer's guarantees are
`nupur_lints`', beside this crate; `rake dylint` loads the two together, so
an `allow` of one of these sits under
`cfg_attr(dylint_lib = "makiri_lints", ...)`, and the self-test's probes
hold the cases of both.
