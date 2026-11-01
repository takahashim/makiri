# Makiri - project guide for Claude Code

Makiri is a Ruby gem: an HTML5 parser + native XPath 1.0 query engine + CSS
selectors, with **no libxml2 / libxslt dependency at any layer**. It parses with
a Rust port of Lexbor's HTML parser into an index-arena DOM (vendored Lexbor
remains as the reference it is tested against) and queries via an original
XPath engine. Security is a first-class goal.

The authoritative design is **`docs/design_doc.ja.md`** (Japanese) - read it
before architectural decisions. This file is the operational summary: the
rules, how to build and test, and where to read more. The reasoning and
history behind each rule are in `docs/internals/`:

| File | What it holds |
| --- | --- |
| `ruby_boundary.md` | panics and unwinding, slots and handles, raising C calls, document guards, `InternalError`, C integers, exports |
| `engine_rules.md` | the fallible-allocation line, a node's `source` offset, pointer-keyed tables |
| `build_and_sanitizers.md` | the ASan preload split, `asan-stack=0`, build gotchas |
| `verification.md` | every rake task annotated (the lints, Verus, Miri, the exhaustive tests, oom, fuzz), `rake diff` |
| `lexbor.md` | why Lexbor is pinned where it is, the `lexbor-contains` guard |
| `layout.md` | the source tree, file by file, and the crate features |
| `subsystems.md` | text input, parsing, indexes, XPath, CSS, serialization, mutation |
| `performance.md` | the numbers, and the decisions not to regress |

The exhaustive API list lives in the code + specs + `CHANGELOG.md`, not here.
When a rule here changes, change its reasoning there in the same commit.

## Hard constraints

- **Vanilla Lexbor, no fork, no patches.** `vendor/lexbor` is a git submodule.
  Never `git apply` to it. Where a port departs from it, the port says so
  (`docs/parser_differences.ja.md`, `compat`), never an edit to Lexbor.
- **No libxml2 / libxslt** anywhere - not linked, vendored, or derived. The
  XPath engine is original. See `NOTICE`.
- **One language: Rust, and no C ships.** The cargo workspace at
  `ext/makiri/rust` is the extension (`makiri_rs`, the Ruby surface), the
  engine (`engine/`, `makiri_engine`: no Ruby, no Lexbor), `falloc/` and
  `nupur/` (the Ruby binding layer, depending on neither; it reaches rb-sys
  only through `nupur/sys/`, `nupur_sys`). The Lexbor
  reference (`reference/`, `makiri_lexbor`) is a workspace of its own, built
  only for the tests. No symbol of ours carries a C-style prefix; the one
  prefix left is `falloc_` on `falloc`'s methods, because a trait method named
  `try_reserve` would be shadowed by std's ABORTING inherent one.
- **Fail closed.** Enforce budgets and caps, validate inputs, and never
  return a truncated or wrong result - raise instead.
- **Export only `Init_makiri` and `ruby_abi_version`.** Any other name in the
  dynamic table can be bound by another gem in the process (it was how
  `nokolexbor` segfaulted against our Lexbor). Nothing else is
  `#[no_mangle]`; on ELF that is the only mechanism that works. Verify with
  `bundle exec rake symbols`, and do not weaken it to "no `lxb_` exported".
- **Every change stays clean under ASan and keeps the fuzzers green.** The
  ASan preload is platform-split (required on Linux, harmful on macOS):
  read `build_and_sanitizers.md` before changing how a sanitized run starts.

### At the Ruby boundary (`ruby_boundary.md`)

- **A panic must not kill the host.** The crate builds with
  `panic = "unwind"`; a method's trampoline (`bridge::rb::method::finish`)
  catches a panic and raises `Makiri::InternalError`. Do not set it back to
  `abort` (`spec/panic_spec.rb`, through `Makiri.__panic(kind)`, would catch
  it). So:
  - whatever must be released across a panic is a `Drop`, never a statement
    after the work;
  - a callback C calls must not panic into C: it latches the panic
    (`caught::PanicLatch`), returns "stop", and the caller re-raises;
  - the GC callbacks (`nupur/src/typed/callbacks.rs`) abort on purpose -
    keep their bodies trivial; the `rb_protect` thunk
    (`nupur_sys::protect::call`, under `raw::call`) holds one C call over
    `Copy` values - keep it so.
- **Nothing that must not unwind can, and the compiler proves it**
  (`rake no_unwind`, `engine/src/no_unwind.rs`). Every `extern "C"` fn and
  GC-run `Drop` body runs through `must_not_unwind` (`rake dylint`'s
  `unguarded_no_unwind_site`). The `self_test` site must be the one left,
  or the check is not live. No `fmt` in post-catch code, no abort call.
- **A VALUE a wrapped object keeps alive is one of its slots** (`nupur::typed`),
  written once by `wrap(ruby, Birth { .. })` and marked pinned. Three objects
  have one slot each (a node wrapper, a Document, a NodeSet); every other
  kept value is a hidden ivar. A payload holds no Ruby value, and no Ruby
  value lives where the stack scan cannot see it (`ruby_value_on_heap`).
- **A payload is reached only through a `typed::Typed` handle**, which holds
  its object until the last borrow. There is no `&Value -> &T`; a type check
  that borrows nothing is `ensure`. An embedded payload (`NodeData`) is read
  by copy (`read`), never borrowed across an allocation. Memory reported to
  the GC goes through a `typed::External`, and `nupur`'s free callback gives
  it back.
- **Every C call that can raise is a wrapper in `nupur/src/raw/`, run under
  `rb_protect` through `raw::call`, returning `Result`.** The C call is
  `nupur_sys::raising`'s, which takes the `&Protected` only `call` lends.
  A raise must never
  `longjmp` over a Rust frame. Which calls raise is classified in
  `lints/makiri_lints/src/raising.rs` (`NON_RAISING`, `REVIEWED`), held to
  the vendored C API raise survey and Ruby's headers; an unclassified
  function is refused. A call that can raise but not under a precondition
  is never a crate-wide entry: it is a `nupur_sys::read` wrapper named for
  the precondition (`enc_get_of_string`), its `allow` at the one C call and
  the precondition its `unsafe` contract, upheld at each call.
- **A raw `VALUE` and `rb_sys::` stay in `nupur_sys` and in `nupur`'s
  `raw/`, `typed/` and the `bridge::rb` value files** (`raw_ruby_outside_bridge`).
  `bridge/` is `#![forbid(unsafe_code)]`; every `unsafe` that touches Ruby
  is `nupur`'s.
- **A Document's content is reached only through a guard** (`bridge::dom`
  over `doc_cell`): an edit while a read is held is `Makiri::Error`, a read
  during an edit is `InternalError`, and an edit of two documents takes both
  writes first. Build a result with the read held (`str_from_read`,
  `node_set_from_read`); no raise can cross a guard.
- **`InternalError` descends from `Exception`, not `StandardError`**, so a
  bare `rescue` passes it through. In `Init_makiri` a failure after
  definition has begun is `fatal`; only the first, Ruby API check is a
  `LoadError` (`run_init`).
- **A C integer is converted only in `nupur_sys::int`** (Nupur's
  `crate::int`). No integer `as` cast and
  no `c_long` / `c_ulong` in Nupur, `bridge`, `glue` or `init`
  (`c_integer_outside_int`): a `long` is 32 bits on Windows. Verus proves
  `int`'s conversions at both widths (`verus/int.rs`); a change goes
  through `rake verus:sync`.
- `clippy::unwrap_used` / `expect_used` / `panic` are on; a site that wants
  one carries an `#[allow]` with its reason.
- **No argument is converted.** A string argument is a `String`, a name a
  `String` or `Symbol`; anything else is `TypeError`. Makiri runs none of the
  caller's Ruby (`runs_callers_ruby`); `NodeSet#each`'s `yield` and
  `XML::Namespace.new` are the two allowed.
- The C-era hardening flags and UBSan are gone, not relaxed: there is no C.

### In the engine (`engine_rules.md`)

- **The engine allocates only through `falloc`**, so `rake oom` can fail each
  site and an OOM raises instead of aborting. Growth is `falloc::VecGrow` /
  `MapInsert` / `SetInsert`; several tables that change together take a
  `falloc::Slot` each first. A plain call is an `#[allow]` with its reason,
  only where the room is known. No std stable sort (it allocates
  infallibly); no engine type holding a heap container derives `Clone`. In
  the shipped crate `infallible_allocation` refuses `format!`, `to_string`,
  `vec!` and kin; an error message is a `Msg` (256 bytes, inline).
- **Recursion carries a `crate::stack::Depth`** (or asks
  `stack::exhausted()`), so a small Fiber stack is refused, not overflowed:
  Ruby's `SystemStackError` would `longjmp` past `Drop`. CSS matching keeps
  its nesting on a heap stack (`spec/native_stack_guard_spec.rb`).
- **XPath holds the GVL for the whole evaluation; parsing releases it.** Do
  not reintroduce a GVL-released XPath path without a full locking story.
  A GVL-released body touches no Ruby (`ruby_in_gvl_released_body`).
- Pointer-keyed tables are a `PtrTable` / `PtrMap` (`crate::ptr_table`),
  whose probing Verus proves; a change goes through `rake verus:sync`.

## Lexbor version (`lexbor.md`)

Pinned to **`05b5d37`** (`v3.0.0-66-g05b5d37`), an untagged master commit,
deliberately. **v3.0.1 is not usable**: it lacks the two CSS-selector fixes
we upstreamed (`#369`, `#371`), so moving to it would regress matching. Check
again on the next tag. This pin changed parsing: `<?php ...?>` is a
`ProcessingInstruction` node (the Standard's new PI tokens). Still vanilla,
never patched.

`:lexbor-contains()` is not supported. No shipped build runs Lexbor's CSS
parser; in the reference builds, `lexbor::contains_guard` keeps every
`lexbor-contains(` from it, and a new call site of Lexbor's parser must go
through the guard.

## Working conventions

- **Commit messages are short**: a subject line and one or two short
  paragraphs, not a long rationale. The why belongs in code comments and
  `docs/`.
- **Pushing is the maintainer's.** Commit locally and hand over the
  `git push` command; do not push, and do not chain `commit && push`.
- **The HTML parser port** (Lexbor to Rust, index-arena DOM) is planned in
  `docs/rewrite_parser.ja.md`; its "進捗" section says where it stands and
  what comes next. Known parser divergences are in
  `docs/parser_differences.ja.md`.

## Build / test

The rake tasks are in `rakelib/*.rake`; `verification.md` annotates each.

```bash
git submodule update --init        # fresh clone only; the reference Lexbor
bundle install
bundle exec rake compile           # the crate alone - the extension links no Lexbor
bundle exec rake lexbor            # the reference Lexbor (cmake); lint, rust:test,
                                   # conformance:parser and the diff fuzzers need it
bundle exec rake spec
bundle exec rake rust:test         # engine (bare and as shipped), falloc, nupur, reference
bundle exec rake nupur:spec        # nupur alone, through its own test extension
bundle exec rake lint              # clippy -D warnings over every crate and config + fmt
bundle exec rake dylint            # the crate's own lints (lints/makiri_lints) + self-test
bundle exec rake unsafe:boundaries # each file's unsafe count and the forbid files
bundle exec rake symbols           # the export gate
bundle exec rake no_unwind         # the unwind-freedom proof
bundle exec rake oom               # OOM-injection and raise-injection sweeps
bundle exec rake diff              # answers vs the recorded C-build baseline
bundle exec rake sanitize          # rebuild with ASan (nightly), run the suite
bundle exec rake debug             # debug_assert! build; leaves it: `rake clean compile`
bundle exec rake fuzz              # robustness fuzzer; `fuzz:sanitize` under ASan
bundle exec rake fuzz:libfuzzer    # cargo-fuzz harnesses (TARGETS=, FUZZ_TIME=)
bundle exec rake invariants        # randomized property checks (spec/invariants/)
bundle exec rake leaks             # macOS leak gate (Linux: spec:valgrind, nightly)
bundle exec rake deny              # cargo-deny over the extension's dependencies
bundle exec rake miri              # falloc + nupur's utf8_copy under Miri
bundle exec rake verus             # Verus proofs (verus/), `verus:sync` checks the copies
bundle exec rake rust:exhaustive   # tests that run every input (decode1 vs from_utf8), release
bundle exec rake bench             # perf vs Nokogiri (runs outside bundle)
bundle exec rake clean             # wipe the build dir
bundle exec ruby -Ilib -r makiri -e 'p Makiri::VERSION'   # smoke load
```

Requires CRuby >= 3.3 and a Rust toolchain; `cmake` only for the reference
Lexbor. `rake sanitize`, `miri` and `fuzz:libfuzzer` need a nightly
toolchain; `rake dylint` needs the one `lints/makiri_lints/rust-toolchain.toml`
pins, with cargo-dylint and dylint-link. `rb_sys` is a RUNTIME gemspec
dependency, since a source install runs `extconf.rb`.

**`rake dylint` lints are the rules above, checked.** A sanctioned exception
is an `allow` with its reason at the site. In the extension
`discarded_ruby_error` is forbidden outright: handle an `Err` with
`Error::rescue`, never `.ok()` / `let _ =`.

**`rake diff` is not an ordinary test.** `spec/differential/baseline/` holds
what the C implementation answered, and cannot be re-recorded. A mismatch is
a finding to investigate, never something to refresh; an intended change
edits the baseline in the same commit as the behaviour change.

## Gotchas (`build_and_sanitizers.md`)

- **A plain `rake compile` after a sanitizer run keeps building with ASan**,
  silently: rake-compiler reuses the Makefile. `rake clean compile` cures
  it; `nm -u lib/makiri/makiri.bundle | grep -c __asan` (0 on a plain build)
  checks it.
- **Run the sanitizer through the rake task**, not `bundle exec rspec`.
- **ASan stack instrumentation is OFF** (`-asan-stack=0`): Ruby raises with
  `__builtin_longjmp`, which leaves stale stack poison. Do not re-enable it
  without solving that.
- A new `.rs` file needs no build step; cargo finds it.
- `spec/fuzz/*.rb` are deliberately not `*_spec.rb`; findings land in
  `spec/fuzz/regressions/` (gitignored).

## Layout (`layout.md`)

```
lib/makiri/              the Ruby API
ext/makiri/rust/         the extension crate (makiri_rs, lib `makiri`) and its workspace
  extconf.rb             cdylib only (an rlib turns LTO off), the link arguments
  src/init.rs            Init_makiri: the class hierarchy
  src/bridge/            the Ruby boundary over nupur; forbid(unsafe_code)
  src/glue/              every Ruby method, one module per feature; unsafe-free
  nupur/                 the Ruby binding layer (read its src/lib.rs first)
    sys/                 nupur_sys: the one crate that depends on rb-sys
  falloc/                fallible allocation; the one `unsafe` outside nupur
  engine/                makiri_engine: no Ruby, no Lexbor, forbid(unsafe_code)
    src/html/            the ported parser (tokenizer/, tree/) and the arena DOM (dom/)
    src/xml/             the XML reader, arena, serializers
    src/xpath/           the XPath 1.0 engine, generic over `Dom`
    src/css_match/       the CSS matcher, over either DOM
    src/selectors/       the ported CSS selector and stylesheet parser
  reference/             makiri_lexbor: Lexbor via bindgen, for the tests only
  fuzz/  lints/  verus/  cargo-fuzz harnesses, makiri_lints, Verus proofs
vendor/lexbor/           git submodule, pinned, never patched
spec/                    rspec; fuzz/, invariants/, differential/ beside it
bench/                   the Nokogiri comparison
docs/                    design docs, parser differences, internals/
```

Features are per crate: the engine has `html-dom` (default), `alloc-inject`
(`rake oom`) and `lexbor-compat` (reference only); the extension has
`alloc-inject` alone; the reference has `css-reference` and
`lexbor-reference`, for the differential fuzz targets.

## Subsystems (`subsystems.md`)

- **Text input.** A document parse decodes bytes as a browser does (the
  Standard's encoding sniffing, `encoding_rs`) and never fails on bad bytes:
  invalid UTF-8 becomes U+FFFD. The programmatic APIs are strict:
  `ruby_verified_text` raises on invalid UTF-8 and on NUL for names,
  selectors, XPath and every engine input; the data family (text, comment,
  attribute values) goes through `ruby_verified_data`, which allows NUL; a
  name given to a factory goes through `ruby_verified_name` (`ArgumentError`).
  Never route a name through the data path, or drop a check. A checked view
  owns a copy of the String's bytes, validated by std's `from_utf8` over the
  copy, never by the cached coderange.
- **Parsing** runs under a `Guard` (`html/tree/guard.rs`): tree depth and
  size against input (`max_tree_depth:`, `max_amplification:`). Keep
  `<select>` building in linear time. `Node#line` is nil rather than wrong.
- **Indexes** (`html/dom/index.rs`) are built lazily, and every edit drops
  them before anything changes (`HtmlParsed::edit`). Every evaluate reads
  the index afresh from the handle.
- **XPath** (`engine/src/xpath/`): per-evaluate budgets (`limits.rs`) fail
  closed with `Status::Limit`. No custom functions; the namespace axis
  raises "not implemented". Name tests are namespace-strict by default
  (`namespace_matching: :lax` is Nokogiri's); HTML element names fold ASCII
  case. Host differences are items of the `Dom` trait, never a host test.
- **CSS** (`css_match/`, `css_cache.rs`, `selectors/`): Lexbor's matcher
  control flow in safe Rust, nesting on a heap task stack, never native
  recursion. Unsupported constructs and over-long chains are found by
  `compile` before matching and raise, never answer empty. A warm query
  allocates nothing but its result (`Scratch`). The engine is per thread.
- **Serialization** is one iterative walk (`html/dom/serialize.rs`) into a
  `ceiling_buf::Buf`; profile `Buf::append` before touching it.
- **Mutation** detaches, never destroys. Every edit goes through
  `bridge::html::edit` then `HtmlEdit::with_tree` (XML: `begin_edit`,
  `Editing::with_arena`); arguments are converted between the two. The
  insertion rules are written once, in `dom_rules::check`.
  `inner_html=` is all or nothing.

## Performance (`performance.md`)

Makiri beats Nokogiri on every `rake bench` row; the numbers are indicative
(runs swing 15-25%), good for catching a regression in kind. Ship only a
measurable win that keeps `rake spec` and `rake fuzz:sanitize` green:
capture a baseline, change one thing, re-bench. `performance.md` lists the
decisions not to regress - the element index for `//tag`, the text index,
`at_xpath`'s first-match path, the compiled-selector cache, and every
Document reporting its arena to the GC.
