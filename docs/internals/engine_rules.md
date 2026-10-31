# Engine rules: fallible allocation, source offsets, pointer tables

- **The fallible-allocation line.** The engine (`html`, `xml`, `xpath`, `css`,
  `selectors`, `css_match`, `css_cache`, `ceiling_buf`) allocates only through `falloc`: `clippy.toml` bans the
  infallible `Box::new` / `Vec::with_capacity` / `reserve`, and every growth too
  (`push`, `insert`, `extend`, `extend_from_slice`, `resize`, a map or set's
  `insert`, `collect`), and `rake oom` fails each site in turn, so an OOM there
  raises instead of aborting. Growth is `falloc::VecGrow` / `MapInsert` /
  `SetInsert`, each a reserve and the op in one call, and `falloc::Slot` for
  an operation that must change several tables together or not at all: take
  every slot first (each a fallible reserve), then fill them - `Slot::push`
  cannot fail, and the slot borrows its vector until filled, so nothing can
  take the room (DOM node + row, the name tables, XPath's namespace and
  variable tables). The old rule,
  "`falloc_reserve` exactly what follows, then the plain call", was a count
  only review checked, and a miscount grew outside the sweep. A plain call
  survives under `#[allow(clippy::disallowed_methods, reason = ...)]` only where
  the room is known and a failure part-way could not be undone (a compaction
  sized to its total, a push back after a pop) - keep those few and say why.
  Not covered: a `clone()` of a type that holds a heap container (clippy
  cannot ban `Clone::clone` per type, so no engine type that holds one
  derives `Clone` - the tokenizer's `Token` did, and no longer does), and what a
  third-party crate allocates inside itself (`encoding_rs`, `chardetng`). The glue's
  Ruby-side storage is Ruby's own memory instead. A TypedData wrapper's struct
  is its object's data, made with it in one call by
  `rb_data_typed_object_zalloc` (`TypedData_Make_Struct`'s function;
  `bridge::typed::wrap_built`) and freed by running the struct's `Drop` in
  place - a non-embedded type's data is `xcalloc`'d, which is also where Ruby
  decides whether malloc pressure calls for a GC, so a Document's arena
  report (`rb_gc_adjust_memory_usage`, which only adds to that pressure) gets
  its collection (`spec/gc_accounting_spec.rb`). `NodeSet`'s node array is a
  `falloc` `Vec`, reported to the GC the same way as it grows and given back
  in its `Drop` - exactly the bytes held, where `ruby_xrealloc2`, which it
  used to be, was counted ~20% over them. Their failure is `NoMemoryError`, Ruby's own, which
  `nupur::raw`'s protected calls hand back as `Err`. An exception message
  allocates nothing at all: a `bridge::rb::Error` holds a `&'static str` or a
  `format_args!` written into `crate::msg::Msg` (256 bytes, inline; longer is
  cut with `...`), and the exception is made only when the trampoline raises
  it - so an error can be made inside an edit's closure, which must run no
  Ruby. What a message quotes is written in by `Display` types that read
  without a call (`bridge::ruby::Describe` / `Display` / `ExceptionText`,
  `msg::Lossy`). `rake dylint`'s `infallible_allocation` holds the whole
  shipped crate to no `format!` / `to_owned` / `to_string` / `vec!` /
  `from_utf8_lossy` / `into_owned` / `CString::new`, by the path each resolves
  to (`lexbor/`, which that build leaves out, is reference code and outside
  the rule); a site that must is `#[cfg_attr(dylint_lib = "makiri_lints",
  allow(infallible_allocation))]` with its reason - one, the `__panic(1)` hook.
  std's STABLE sorts (`sort`, `sort_by`, `sort_by_key`, `sort_by_cached_key`)
  are banned there too: they take a scratch buffer from the global allocator
  and abort on OOM, invisibly to `rake oom`. Sort in place (`sort_unstable_by`)
  or, for document order, `xpath::order`'s natural merge sort, which takes its
  scratch from falloc and falls back to the in-place sort without it.
- **A node's `source` field holds its start tag's byte offset** (offset + 1;
  0 for none), for `Node#line`: `Document::set_source_offset` /
  `forget_source_offset` / `source_offset` (`html/dom/mod.rs`) are its only
  writers and reader. A copy within the document keeps it; one from another
  document drops it, since it indexes that document's source
  (`html/dom/copy.rs`).
- Pointer-keyed
  tables hash with `crate::ptr_table::ptr_hash`, and are a `PtrTable` (sized
  once) or a `PtrMap` (grows) rather than another hand-written probe loop -
  Verus proves the probe always stops (`verus/ptr_table.rs`), so a change to
  either table goes through `rake verus:sync`; a
  key that can be 0 (an XML token) supplies its own empty marker through
  `TableKey`. An XPath step's name test is compiled once into a
  `nodetest::CompiledTest` - where an unknown prefix is reported, via
  `Names::resolve_prefix` - and axis walks report through `ControlFlow`.
