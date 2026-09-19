# Makiri - project guide for Claude Code

Makiri is a Ruby gem: an HTML5 parser + native XPath 1.0 query engine + CSS
selectors, with **no libxml2 / libxslt dependency at any layer**. It parses via
vendored Lexbor and queries via an original XPath engine. Security is a
first-class goal.

The authoritative design is **`docs/design_doc.ja.md`** (Japanese) - read it
before architectural decisions. This file is the operational summary: constraints,
how to build/test, the subsystem map, and the non-obvious gotchas. The exhaustive
API list lives in the code + specs + `CHANGELOG.md`, not here.

## Hard constraints

- **Vanilla Lexbor, no fork, no patches.** `vendor/lexbor` is a git submodule
  pinned to a release **tag**. Never `git apply` to it. Lexbor gaps are absorbed
  in `ext/makiri/rust/src/lexbor/adapter/`, never by editing Lexbor.
- **No libxml2 / libxslt** anywhere - not linked, vendored, or derived. The
  XPath engine is original. See `NOTICE`.
- **One language.** The extension is a single Rust crate
  (`ext/makiri/rust`); the only C that ships is vendored Lexbor, which keeps its
  `lxb_*` names. Nothing of ours carries a C-style prefix any more: the crate
  exports only `Init_makiri` and `ruby_abi_version`, so every other item is
  named as Rust. (The `mkr_` prefix was the C ABI's symbol convention and went
  with it.)
- **Security-first / fail-closed.** Enforce per-evaluate XPath budgets and
  node-set caps, validate inputs, never return a truncated/wrong result (raise
  instead). **Export only `Init_makiri`** (plus `ruby_abi_version`, which Ruby
  reads at require time): the vendored Lexbor archive is built with default
  visibility, so a bundle that re-exports its ~1700 `lxb_*`/`lexbor_*` symbols
  lets another Lexbor-based gem in the same process (e.g. `nokolexbor`) bind its
  `lxb_*` calls to our different Lexbor version → segfault. Keep Makiri's Lexbor
  private; **verify with `bundle exec rake symbols`**, which asserts the whole
  claim (nothing of ours or Lexbor's left undefined, and nothing but those two
  names exported). Do not weaken that gate to "no `lxb_` exported" - it passed
  happily while ~220 `mkr_*` names leaked into the dynamic table.
  Every change must stay clean under ASan and keep the fuzzers green. The ASan
  runtime preload is PLATFORM-SPLIT (required on Linux, harmful on macOS) - see
  "AddressSanitizer: the preload is platform-split" below before changing how a
  sanitized run is launched.

- **A panic must not kill the host.** The crate builds with `panic = "unwind"`,
  so magnus's `catch_unwind` turns a panic into Ruby's `fatal`: on the thread
  that ran the query, which dies alone while the process keeps working, with
  `ensure` and `at_exit` run and every `Drop` executed on the way out. `abort`
  did none of that - no destructor, no cleanup, SIGABRT for the whole process -
  so **do not set it back**; `spec/panic_spec.rb` is what would catch that,
  through the `Makiri.__panic(kind)` hook that exists for no other purpose.
  Two rules follow, and anything new has to keep both.

  **Whatever must be released across a panic is a `Drop`**, not a statement
  after the work - a plain `lxb_*_clean` following a parse is exactly what
  unwinding skips. `lexbor::selectors::Session` and `post_parse::DocOwner`
  are the two that had to be converted, both around process-global or
  not-yet-owned Lexbor state.

  **A callback must not panic INTO C.** Rust turns an unwind at an `extern "C"`
  boundary into an abort, because unwinding through frames built without unwind
  tables is undefined behaviour - so the callback catches the panic, latches it
  and returns the stop status, and the caller re-raises once C has unwound.
  `caught::PanicLatch` is that, and it is deliberately the same shape the
  callbacks already used for the node cap and OOM. It is installed in all nine:
  the CSS traversal (`find_cb`/`first_cb`/`match_cb`), both serializer sinks,
  the tokenizer's `pos_token_cb`, `bridge::gvl`'s trampoline (which carries the
  whole parser), and - covering every `rb_protect` at once, since magnus runs
  the closure inside its own `extern "C"` trampoline - `bridge::ruby::protect`.
  Do not call magnus's `protect` directly; ours is the one with the latch.
  Two `extern "C"` functions have no latch ON PURPOSE. The GC callbacks in
  `bridge/typed.rs` run where nothing can be raised and a half-freed object is
  worse than a stop, so they keep aborting - keep their bodies trivial. The two
  raw `rb_protect` thunks (`exception_message_thunk`, `strict_transcode_thunk`)
  contain only C calls, so there is no Rust there to panic; keep it that way.

  **An entry point exposed to untrusted input raises `Makiri::InternalError`,
  not `fatal`.** `bridge::ruby::entry` wraps the twenty-four methods a crafted
  document or expression reaches - parse and fragment, xpath/at_xpath/evaluate,
  css/at_css/matches?, the serializers, the text readers - and turns a panic
  there into that exception. It descends from `Exception`, NOT `StandardError`,
  which is the point: a bare `rescue => e` keeps passing it through, because a
  broken invariant is not a bad selector, while a host that wants to turn one
  request into a 500 can catch it WITHOUT a thread boundary to re-raise at (a
  `fatal` cannot be rescued in its own frame at all). Everywhere else a panic
  stays `fatal`, which is the right severity on a path nobody's data reaches.
  Wrap a new entry if it parses, evaluates, or walks a tree built from input.

  `clippy::unwrap_used` and `clippy::panic` (in `Cargo.toml`) keep a new panic
  from arriving by accident; a site that wants one carries an `#[allow]` with a
  reason. `spec/panic_spec.rb` drives `Makiri.__panic(kind)`: kind 4 panics
  below the GVL-release frame, the case that proves the latch since without it
  that one aborts, and kind 5 goes through `entry`.

  The C-era hardening flags (`-D_FORTIFY_SOURCE=2`, `-fstack-protector-strong`,
  `-fvisibility=hidden`, `-Wformat-security`) are **gone rather than relaxed**:
  they hardened C sources, and there are none. `-fvisibility=hidden`'s job is
  the one that survives, and it is enforced AT THE SOURCE: nothing but
  `Init_makiri` is `#[no_mangle]`, so rustc emits no other exported name. That
  is not a stylistic choice - on ELF it is the only thing that works. rustc owns
  the cdylib link and passes its own export list, a second `--version-script` is
  MERGED rather than applied (so it cannot narrow), and `objcopy`/`strip` cannot
  remove an entry from a linked `.so`'s `.dynsym` at all. The post-link trim in
  `extconf.rb` survives as belt-and-braces on macOS, where `strip -u -r -s` does
  work; do not mistake it for the mechanism.
  UBSan is gone for the same reason: rustc's `-Zsanitizer` has no `undefined`,
  and there is no C left for `-fsanitize=undefined` to instrument.

## Lexbor version

Pinned to **`3a2d595`** (`v3.0.0-25-g3a2d595`, builds cleanly with our
`LEXBOR_BUILD_SHARED=OFF` config). **Normally we pin to a release tag**, but this
is an *untagged master* commit taken deliberately: the latest release tag is
still v3.0.0, and master carries fixes Makiri needs, including **two CSS-selector
fixes we upstreamed** - `#369` (`3a2d595`: class/ID selectors now match
case-sensitively except in quirks mode, like browsers, instead of always
case-insensitively) and `#371` (`940162d`: a prefix-less type selector no longer
defaults to the universal namespace) - plus a **heap-overflow fix in the
`:lexbor-contains()` parser** (`8a14bc0`, reached via `Node#css`), the `#365`
tokenizer size-limit (DoS) fix, `<select size>` NULL-deref, ruby `rp`/`rt`
parse-error, HTML scope/attribute fixes, and encoding/URL memory fixes. All are
bugfixes (no feature/breaking churn). **Move back to a release tag** as soon as
one ships after v3.0.0 (it should contain all of the above) via `git submodule
update`; until then, keep this pin. Still **vanilla, NEVER patched** - the
constraint that relaxed is "release tag only", not "no fork".

## Build / test

```bash
git submodule update --init        # fresh clone only
bundle install
bundle exec rake compile           # builds vendored Lexbor static lib, then the crate
bundle exec rake spec
bundle exec rake clean             # wipe the build dir (regenerates the Makefile next compile)
bundle exec rake clean:lexbor      # wipe vendor/lexbor/{build,dist} (full Lexbor rebuild)
bundle exec ruby -Ilib -r makiri -e 'p Makiri::VERSION'   # smoke load

bundle exec rake symbols           # THE export/undefined gate - see Hard constraints
bundle exec rake diff              # answers vs the recorded C-build baseline (see below)
bundle exec rake sanitize          # rebuild w/ -Zsanitizer=address (nightly), run suite
bundle exec rake fuzz              # robustness fuzzer (spec/fuzz/); FUZZ_ARGS to tune
bundle exec rake invariants        # randomized property checks (spec/invariants/):
                                   # namespaces, tree shape, index staleness,
                                   # serialization, the text-input contract.
                                   # INVARIANT_COUNT tunes the sweep;
                                   # `invariants:sanitize` runs them under ASan
bundle exec rake fuzz:sanitize     # fuzz under ASan - the engine's memory-safety net
bundle exec rake fuzz:libfuzzer    # cargo-fuzz harnesses (needs cargo-fuzz + nightly);
                                   # TARGETS=css,html_xpath narrows, FUZZ_TIME per
                                   # target or FUZZ_BUDGET total. ASan on Linux only:
                                   # an ASan harness deadlocks in dyld on macOS, so
                                   # macOS runs `-s none` (see the Rakefile)
bundle exec rake leaks             # macOS malloc-leak gate (ASan runs detect_leaks=0,
                                   # so this is the ONLY leak check; flags per-call
                                   # leak stacks through the ext incl. rescued raises)
bundle exec rake oom               # OOM-injection sweep: rebuilds with
                                   # MAKIRI_ALLOC_INJECT=1 and fails each core alloc
                                   # site in turn - every OOM branch must fail closed
                                   # (clean raise or baseline-identical result)
bundle exec rake "sanitize:lexbor" # also build vendored Lexbor under ASan+UBSan (mraw-arena
                                   # overflows). LINUX ONLY: Apple clang's ASan ABI
                                   # does not match rustc's runtime (load segfault)
bundle exec rake kani              # Kani proofs over the Ruby-free core (needs cargo-kani).
                                   # The successor to the C-era CBMC harnesses: the
                                   # allocator, cbuf, UTF-8 validate/decode.
bundle exec rake bench             # perf vs Nokogiri (bench-only gems; runs outside bundle)
```

Requires CRuby >= 3.2, `cmake`, and a **Rust toolchain** (`cargo`). A source
install needs cargo too; that is why `rb_sys` is a RUNTIME gemspec dependency -
`extconf.rb` requires it at install time. Binary gems are published per platform,
so most users never compile.

**`rake diff` is not an ordinary test.** `spec/differential/baseline/` holds what
the **C implementation answered**, recorded while it still existed, and the probes
compare this build against it. It cannot be re-recorded - there is no second
implementation left - so a mismatch is a finding to investigate, never something
to refresh. If a difference is genuinely intended, edit the baseline in the same
commit as the behaviour change, so it reviews as a behaviour change rather than
as a regenerated blob. It is the check that caught an ASCII-8BIT string, a quoted
error message and a lost NUL terminator that the 1001-example suite passed over.

### AddressSanitizer: the preload is platform-split

`rake sanitize` works. It did not for a long while - the extension segfaulted at
address 0 during `require` - and the cause is worth keeping, because the fix is
the *absence* of something the task used to do.

rustc links its own ASan runtime into the cdylib as an `@rpath` dependency, so
dyld loads it together with the extension. The task used to ALSO preload a
runtime, and every way of doing that is broken on macOS:

- Preloading Apple's clang runtime (what `asan_runtime_path` found, via `cc
  -print-file-name`) hands the process a runtime that does not export
  `__asan_version_mismatch_check_v8` - which rustc's instrumentation calls from
  every image's `asan.module_ctor`. Under `-undefined dynamic_lookup` a missing
  symbol is not a link error but a NULL pointer, so that constructor jumped to 0
  while dyld ran the image's initialisers, **before `Init_makiri`**. That was the
  load segfault - the same `dynamic_lookup` hazard recorded two bullets above,
  in a symbol nobody was auditing.
- Preloading rustc's runtime instead deadlocks inside dyld: its init takes a
  non-recursive spin lock, maps shadow memory, and the dyld call that does so
  allocates - re-entering that same init through ASan's own malloc interceptor,
  which then spins in `sched_yield` forever. No output, 100% CPU, before Ruby
  executes a line.
- Preloading clang's alongside rustc's linked one is simply two runtimes.

So on macOS the task preloads nothing, and passes `verify_interceptors=0` (with
`verify_asan_link_order=0`, the same assertion under another name). That check
asserts the runtime loaded ahead of libSystem, which is false for a library dyld
brings in with the extension; the interceptors themselves install fine - a
`verbosity=1` run reports "libc interceptors initialized" with the shadow
mapped, `redzone=16` and a 256M quarantine - so heap red-zoning is live. Do not
"fix" that flag away. Dropping the macOS preload also closed a coverage hole:
three `spec/xml_html_boundary_spec.rb` examples used to skip under ASan because
a subprocess cannot inherit `DYLD_*`. `ASAN_OPTIONS` is an ordinary variable, so
they run now.

**Linux is the mirror image, and generalising from macOS is the trap.** rustc
links the sanitizer runtime into executables but NOT into a cdylib, so the `.so`
carries `__asan_*` undefined and expects the host to supply them. With no
preload `dlopen` fails outright - `undefined symbol: __asan_handle_no_return` -
and no `ASAN_OPTIONS` value helps, because the flags govern checks, not symbol
resolution. So Linux keeps `LD_PRELOAD` of GCC's `libasan`, which exports the
same `_v8` ABI rustc asks for (Apple's runtime is the odd one out, not
`libasan`). `Rakefile`'s `asan_preload_env` is the single place that decides,
and it returns `{}` on macOS by construction.

Verified on macOS (arm64): `rake sanitize` builds and completes the suite, 1000
examples, 0 failures. The Linux half was verified in a linux/amd64 container on
the mechanism rather than on Makiri: an ASan-instrumented cdylib `dlopen`'d by an
uninstrumented host fails to load without the preload, loads with it, and with it
reports a real heap-buffer-overflow. CI is what exercises it on Makiri itself.

Two things the earlier investigation recorded as ESTABLISHED were wrong. They are
corrected here so nobody re-derives them: `lr` **is** inside the bundle's `r-x`
text (the dump's memory map has ~7900 entries, dozens of them `rw-` ranges named
`makiri.bundle`, which is what misled the reading), and the null-call hypothesis
was right rather than dead - the NULL symbol was an `__asan_*` one, waved through
by the check that concluded "every undefined symbol is legitimate".

### Build / runtime gotchas (read before debugging weirdness)

- **A plain `rake compile` after a sanitizer run keeps building with ASan.**
  extconf writes `tmp/<platform>/makiri/<ruby>/Makefile` with
  `RB_SYS_EXTRA_RUSTFLAGS ?= -Zsanitizer=address ...`, and rake-compiler re-runs
  extconf only when that Makefile is ABSENT - so every later `rake compile`
  silently reuses the sanitized flags. Nothing says so: the build is quiet, and
  the first symptom is `rake spec`/`rake diff` dying with "Interceptors are not
  working" (or, before the preload fix, something stranger). `rake clean compile`
  is the cure, and `nm -u lib/makiri/makiri.bundle | grep -c __asan` (0 on a
  plain build) is the check. This is worth knowing because it invalidates
  measurements taken in between without failing anything.

- **Adding a source file needs no special step.** cargo discovers modules from
  `mod` declarations and does its own dependency tracking, and the Makefile
  extconf writes re-runs cargo on every build - so a new `.rs` cannot be silently
  dropped the way a new `.c` could. (That hazard was real and is worth
  remembering for anything else generated at configure time: extconf globbed
  sources once, a stale Makefile omitted the new file, and macOS's `-undefined
  dynamic_lookup` turned the missing symbols into runtime NULL jumps rather than
  a link error.)
- **Sanitizer must be run via the rake task, not `bundle exec rspec`.**
  `MAKIRI_SANITIZE=address` makes extconf build the crate with
  `-Zsanitizer=address` on the **nightly** toolchain (it aborts if nightly is
  missing rather than silently building it plain). It preloads the ASan runtime
  on Linux and NOT on macOS - see "AddressSanitizer: the preload is
  platform-split", and do not "simplify" the two into one - plus
  `verify_interceptors=0`/`verify_asan_link_order=0`. `ASAN_OPTIONS` also disables
  LSan/container/odr checks (Ruby+Lexbor are uninstrumented); heap errors in our
  code still fire. CI runs a separate `sanitize` job on Linux. There is no
  `undefined` mode any more - see Hard constraints.
- **ASan *stack* instrumentation is deliberately OFF in sanitize builds**
  (`-Cllvm-args=-asan-stack=0`, passed by extconf). CRuby is
  built with `RUBY_SETJMP = __builtin_setjmp`, so a raise unwinds via
  `__builtin_longjmp`, which ASan cannot intercept: a raise crossing an
  instrumented frame (ours, or a Ruby raise through `rb_protect` under the
  evaluator) leaves that frame's stack-redzone poison behind, and a later
  interceptor (memcpy & co.) in the uninstrumented interpreter trips over the
  stale shadow - a layout-sensitive spurious report that ASan then aborts on
  while rendering (`asan_thread.cpp` `kCurrentStackFrameMagic` CHECK; this took
  CI's sanitize jobs down via the XML-mutation PBT, which raises thousands of
  times - see `docs/ci-crash/INVESTIGATION.md`). Heap red zones and the
  `xml::arena` poisoning are unaffected; only stack-buffer checks are lost.
  Do not re-enable without solving the `__builtin_longjmp` shadow problem.

  What used to soften this loss no longer applies and was not replaced:
  `-fstack-protector-strong` covered stack smashing in the C, and there is no C.
  Rust's own bounds checking is what covers the class now on every path that is
  not `unsafe`, which is why `unsafe` blocks carry a stated contract.
- **Vendored Lexbor is built with LTO on macOS and on Linux, but NOT on mingw** -
  that split is not a preference, it is what each platform's linker can read.
  Measured against the same build without it, once the GC accounting above made
  the numbers stable (before that the parse row swung 1.4-2.3x and nothing here
  was quotable): **parse -16.2%**, **`to_html` -9.2%**, and **`css` +5.3%** -
  that last one is a real regression, not noise, and it is the price. About two
  seconds of link. It was the only compile-option win available: Lexbor is ALREADY
  `-O3`, because `CMAKE_BUILD_TYPE=Release` appends `-O3 -DNDEBUG` after
  Lexbor's own `-O2` and the last `-O` wins - reading its
  `LEXBOR_OPTIMIZATION_LEVEL` default as the effective level is a trap.

  Enabling it everywhere was tried and CI gave three different answers, so do
  not re-derive them: **darwin** ld64 reads the bitcode natively and both the
  extension and `cargo test` link; **linux** links the extension (gcc drives it,
  with its LTO plugin) but `cargo test` goes through rust-lld, which cannot read
  GCC's GIMPLE at all; **mingw** fails outright, because cmake indexes the
  archive with plain `ar`, which records no LTO symbols.

  Linux therefore gets LTO only with the WHOLE chain in LLVM - clang to emit
  bitcode, `llvm-ar`/`llvm-ranlib` to index it, and clang+lld to drive the
  extension's link (`cargo test` already uses rust-lld, which reads bitcode).
  extconf DETECTS that chain rather than requiring it, probing lld by actually
  linking with it, so a gcc-only machine builds exactly as before; CI installs
  `clang lld llvm` so the fast path is the one it exercises. mingw stays out:
  its Ruby is MinGW-gcc-built, so bringing clang in is an ABI question, not a
  flag.

  `-march=native` is deliberately NOT used. It measured no gain at all - the hot
  code is a byte-at-a-time state machine plus libc's already-dispatched
  memcpy/memset, with nothing for the compiler to vectorise - and it would bake
  the CI runner's ISA into a gem that has to run on the user's CPU. `-flto=thin`
  measured the same as `-flto`, so the choice between them is only about which
  compiler spells it.

  NOT applied under the sanitizer (whole-archive inlining only makes a report
  harder to read). `MAKIRI_LEXBOR_NO_LTO=1` opts out, and the install stamp
  tracks it (`plain` / `plain-lto` / `plain-lto-llvm` / `asan`), so a mode switch rebuilds - but
  only through a path that re-runs extconf, i.e. `rake clean compile`, per the
  Makefile note above.

- **A plain `sanitize` build does NOT catch overflows inside Lexbor's `mraw`
  bump arena.** A sub-allocation overrunning into the next one stays within one
  malloc'd chunk, so the heap allocator's red-zones never see it (this is exactly
  how the v3.0.0 `:lexbor-contains()` overflow hid from ASan). To catch that
  class, build Lexbor *itself* under ASan: `MAKIRI_SANITIZE_LEXBOR=1` makes
  extconf pass `-DLEXBOR_BUILD_WITH_ASAN=ON` (Lexbor's mraw is ASan-aware - it
  poisons the arena and unpoisons each allocation, so an intra-arena overrun
  writes into poisoned memory and ASan reports it). Drive it with `rake
  "sanitize:lexbor"` (slow: full instrumented Lexbor rebuild; FUZZ_ARGS routes to
  the fuzzer). extconf stamps the Lexbor install mode (`plain`/`asan`) and
  auto-rebuilds on a switch, so an instrumented Lexbor never leaks into a normal
  build. Switching the Lexbor *commit* still needs `rake clean:lexbor` (the stamp
  tracks mode, not revision). No Lexbor patch - it is a vendor build flag.
- **Our XML bump arena (`src/xml/arena.rs`) is ASan-red-zoned, so its intra-arena
  overflows ARE caught** - the same blind spot as Lexbor's mraw, but this is our
  own module. The allocator poisons each fresh 64 KiB chunk and unpoisons only
  the bytes a cut hands out (the `[size, need)` alignment tail stays poisoned),
  so a write past one node/bytes/scratch cut hits poisoned memory and ASan
  reports it. It auto-activates under any address-sanitized build - no extra
  flag, unlike Lexbor - and is a no-op otherwise. So plain `rake sanitize` /
  `fuzz:sanitize --target xml,mutate` already cover the arena. Everything else
  we write allocates through `falloc` onto the system allocator, or - for the
  glue's Ruby-side storage - through Ruby's xmalloc; ASan red-zones both per
  allocation - no arena, no special handling. Keep the unpoison at exactly the requested `size` (not
  `need`); widening it to `need` would silence off-by-one-into-padding
  overflows.
- **The fallible-allocation line.** The engine (`xml`, `xpath`, `css`,
  `lexbor/adapter`, `cbuf`) allocates only through `falloc`: `clippy.toml` bans the
  infallible `Box::new` / `Vec::with_capacity` / `reserve`, and `rake oom` fails
  each site in turn, so an OOM there raises instead of aborting. The glue's
  Ruby-side storage - TypedData wrappers (`bridge::ruby::wrap_zeroed`) and
  `NodeSet`'s node array - uses Ruby's `ruby_xmalloc` family instead: its failure
  is `NoMemoryError`, Ruby's own, and because that raise longjmps, it may happen
  only in a frame that owns nothing or under `rb_protect` (`value_to_ruby`).
  std's STABLE sorts (`sort`, `sort_by`, `sort_by_key`, `sort_by_cached_key`)
  are banned there too: they take a scratch buffer from the global allocator
  and abort on OOM, invisibly to `rake oom`. Sort in place (`sort_unstable_by`)
  or, for document order, `xpath::order`'s natural merge sort, which takes its
  scratch from falloc and falls back to the in-place sort without it.
- **`node->user` is reserved** for source-location byte offsets (see below) - do
  not repurpose it. Its encoding (offset + 1) lives in `HtmlNode::source_offset`
  / `stamp_source_offset`, the only reader and writer.
- **Lexbor's DOM structs are read only through `lexbor::adapter::html`'s typed
  handles** - the index builders included; its two tree writes are named
  (`HtmlAttr::backfill_parent`, `HtmlNode::stamp_source_offset`). Pointer-keyed
  tables hash with `crate::ptr_table::ptr_hash`, and are a `PtrTable` (sized
  once) or a `PtrMap` (grows) rather than another hand-written probe loop; a
  key that can be 0 (an XML token) supplies its own empty marker through
  `TableKey`. An XPath step's name test is compiled once into a
  `nodetest::CompiledTest` - where an unknown prefix is reported, via
  `Names::resolve_prefix` - and axis walks report through `ControlFlow`.
- The fuzzer's `spec/fuzz/*.rb` are deliberately not `*_spec.rb`, so `rake spec`
  ignores them; findings land in `spec/fuzz/regressions/` (gitignored).

## Layout

```
lib/makiri/                Ruby API (Document, Node, Element, NodeSet, XPathContext, ...)
ext/makiri/rust/           the extension: one crate, package makiri_rs, lib `makiri`
  extconf.rb               builds vendored Lexbor (cmake), then hands the crate to
                           cargo via create_rust_makefile; owns the link arguments
                           and the export trim
  build.rs                 bindgen over Lexbor's own headers -> the layout and
                           constants the crate reads (never transcribed by hand)
  src/
    init.rs                Init_makiri: the class hierarchy and the registration seam
    falloc/                fallible allocation - every engine allocation goes
                           through here, so `rake oom` can fail it and OOM raises
                           rather than aborting the host process (the glue's
                           Ruby-side storage is Ruby's xmalloc; see the gotchas)
    cbuf.rs                `Buf`: the owned, capped, growable byte buffer
    cutf8.rs               the one UTF-8 validator + strict 1-codepoint decoder
    bridge/                the Ruby boundary - the ONLY layer allowed raw Ruby String
                           access (RSTRING) and verified-string minting, and where
                           raising C calls (rb_String, typed-data checks) and the
                           wrap-then-store constructor live; a raise becomes `Err`
                           there. `rake unsafe:boundaries` pins that boundary:
                           the crate denies `unsafe_code` (`lib.rs`), so every
                           file that needs it carries an `allow` and the script
                           holds each one's count exactly - a new file fails even
                           where a parent module's `allow` kept rustc quiet - plus
                           the 56 `forbid` files. `glue/**`, `xpath/**` and
                           `css/**` are fully safe (their module roots carry
                           `#![forbid(unsafe_code)]`, which the gate pins), and
                           `rb_sys::`, `Value::from_raw` and raising C calls are
                           0 outside `bridge/`
    glue/                  Ruby <-> engine surface, one module per feature
                           (node/doc/node_set/xpath/html_node/xml_node); all
                           `unsafe`-free, its wrappers and TypedData live in
                           `bridge/`
    xpath/                 native XPath 1.0 engine, generic over a `Dom` trait;
                           `#![forbid(unsafe_code)]` and Lexbor/Ruby-free. The
                           two `Dom` instances live with their layers:
                           `lexbor/xpath.rs` (HTML) and `xml/xpath.rs` (XML),
                           joined for the glue by the `Cx` enum in
                           `bridge/xpath.rs`
    xml/                   native XML reader (Ruby/Lexbor-free; own arena), plus
                           its XPath `Dom` instance
    lexbor/                the Lexbor boundary: `abi` - the generated layout and
                           functions (the `_noi` twins included), the ONE place
                           a Lexbor function is declared (a second `extern "C"`
                           spelling is a second Rust type for the symbol;
                           `rake unsafe:boundaries` fails on one). Only the three
                           exports no header declares are written by hand, and
                           build.rs's `UNDECLARED_EXPORTS` fails the build if
                           their C definitions change - `adapter`, the one reader of
                           Lexbor's DOM structs, plus the attr->owner index,
                           text index, source location and post-parse - and the
                           selectors/stylesheet/serialize/fragment facades, the
                           CSS selector parser (`css_parser.rs`) and the XPath
                           HTML backend (`xpath.rs`); every `lxb_*`/`Lxb*` name
                           outside it is 0
    css/                   CSS selector lowering over the lexbor-owned selector
                           parser (safe Rust, no Lexbor ABI names)
  fuzz/                    cargo-fuzz harnesses (xml/html, xpath/xml_xpath/
                           html_xpath, css; built on PRs, run nightly)
vendor/lexbor/             git submodule, pinned 3a2d595 (v3.0.0-25), NEVER patched
spec/fuzz/                 grammar-aware robustness fuzzer
spec/invariants/           randomized property checks (see its README)
spec/differential/         the recorded C-build answers + the probes (see `rake diff`)
bench/                     Nokogiri-comparison benchmark
docs/design_doc.ja.md      authoritative design (read this)
```

Three features, one per layer, and the default is the extension: **`ruby`** (the
magnus boundary + `glue` + `init`; implies `lexbor`), **`lexbor`** (the layers
that read Lexbor's DOM: the generated ABI, `css`, `lexbor/adapter`, the XPath HTML
instance) and **`alloc-inject`** (the `rake oom` hook, off in any normal build).
The engine - `xml`, `xpath`, `falloc`, `cbuf`, `cutf8` - is behind no gate at
all. So the fuzz crate builds `--no-default-features --features lexbor` and Kani
builds `--no-default-features`. The ~30 features that used to stand here were
migration scaffolding, one per ported C file, and went with the C.

## Subsystems

**Text-input contract.** Parsing **honours the input String's encoding**
(`ruby_to_utf8`, `bridge/string.rs`): UTF-8 / US-ASCII / ASCII-8BIT pass
through untouched (the UTF-8 common case is a single encoding compare - no
transcode, no copy), any other encoding (Shift_JIS, EUC-JP, ISO-8859-1, ...) is
`rb_str_encode`'d to UTF-8 (invalid/undef → U+FFFD) so its content survives
instead of being read as raw UTF-8. After that the bytes are UTF-8. **HTML
parsing then decodes leniently like a browser**: `utf8_sanitize`
(`lexbor/adapter/utf8_input.rs`) replaces any remaining invalid UTF-8 with U+FFFD (a NUL is left
for the HTML5 tokenizer to drop/replace), so parse/fragment **never fail** on
bad bytes and the DOM is always valid UTF-8. The validation is a dedicated
validate-only scan (Unicode well-formed table + word-at-a-time ASCII); it is
skipped entirely when the String's cached coderange (read via `ENC_CODERANGE`,
no forced scan) already proves it valid - `parse_html`'s `assume_valid` and
`ruby_str_known_valid_utf8`. The **programmatic APIs are strict**:
`verify_text` (`bridge/string.rs`) raises `Makiri::Error` for **invalid
UTF-8 everywhere** at the XPath/CSS/mutation boundaries (expr, selector,
attribute name/value, `content=`, `name=`, `create_*`, variable/namespace) -
never truncate/repair. **Embedded NUL (U+0000) is a two-tier contract**: rejected
for names/tags/namespaces/PI target+data/selectors/XPath/variables and all engine
inputs (which assume NUL-terminated C strings), but **accepted for the HTML
data-family** - text/comment node content (`create_text_node`/`create_comment`/
`content=`) and attribute values (`[]=`/`set_attribute_ns`) - so the DOM can hold
U+0000 like browsers. Those data-family sites go through `ruby_verified_data`
(distinct type `RubyData`, UTF-8-validated but NUL-permitting;
consumed only as `(ptr,len)`), never `verify_text`. `Makiri::XML` keeps
rejecting NUL everywhere (its `crate::xml` engine enforces the XML 1.0 char class,
independent of the bridge; U+0000 can't be well-formed XML). Don't drop the
UTF-8 checks or route a name/engine string through the data path; see
`docs/string_types.md`.

**Parsing & source location** (`lexbor/adapter/post_parse.rs`, `source_loc.rs`).
`parse_html` drives Lexbor's low-level pipeline (`parser_create`/`init` →
`parse_chunk_begin` → override the tokenizer's token-done callback, **chaining**
the parser's tree builder → `chunk_process`/`chunk_end`) so it can record each
element start-tag's byte offset (`token->begin`). After the tree is built,
`pos_assign_to_dom` walks pre-order, matches each element to the next
recorded token by tag id (bounded lookahead), and stamps `offset+1` into
`node->user`; a line table (`source_loc::Lines`, built once) resolves that to a
1-based line. `Node#line` returns an Integer, or **nil** when unplaceable
(parser-inserted implicit html/head/body, text/comment/attribute nodes) - never
a wrong line. Recorder bounded by `source_loc::MAX_TOKENS` (fail closed → nil, never
wrong). The document outlives `lxb_html_parser_destroy` (it only unrefs
tkz/tree). An earlier `line: :text`/`:none` option was removed - `:text` (a
separate source scan) measured *slower* (~36%) and was only approximate.

**The stamping is LAZY, and that is load-bearing for parse speed.** Recording
rides the parse (`pos_token_cb`, ~1.7% of it), but `pos_assign_to_dom` - the
walk that pairs elements with tokens - profiled at **11% of a parse**, paid by
every caller for an answer most never ask for. So the parse hands the offsets
back (`source_loc::Positions`, which drops the Recorder's pointer INTO the
source buffer, since the offsets were already resolved) and `HtmlParsed::pending_pos`
holds them. `HtmlParsed::assign_positions` does the walk once, on the first
`#line` - or on the first MUTATION, via `ensure_document_mutable`, which is the
last moment the tree is still the one the parser built. That second trigger is
what keeps the answers identical to stamping eagerly; a walk over an edited tree
would pair elements with the wrong tokens. Do not move it after the edit, and do
not skip it: `spec/source_location_spec.rb`'s "deferred stamping" examples and
the `lines` differential probe are what catch either. Measured by profile, the
change took Makiri's own share of a parse from 13.3% to 2.6%. `lines_build`
stays eager (~2%): deferring it would mean holding the source buffer, which is
the one thing the parse frees.

**attr→owner index** (`lexbor/adapter/dom_index.rs`). Lexbor never links an
attribute back to its element, so we build a `PtrTable` (pointer
keys, lazy two-phase build - count, size once, fill; iterative DFS, no recursion
→ no stack DoS; OOM fails closed and retries). The build also **backfills each
attribute's `node.parent`** to its owner (safe: Lexbor walks the tree via
first_child/next, never attr.parent), so the XPath engine handles
parent/ancestor axes and document-order over attributes with no special-casing.
Owned by the parse handle (`HtmlParsed::dom_index`, `DomIndex::owner_of`);
`HtmlParsed::invalidate_indexes` drops it after any mutation so it rebuilds on the
next query. The same walk **co-builds
an element index** (`tag id → elements`, document-order CSR) used by the XPath
`//tag` fast path; only Lexbor's static tag-id range `[1, LXB_TAG__LAST_ENTRY)`
is bucketed - custom-element tag ids are *pointer values* (`lxb_tag_append`),
so those elements are left out and `//customtag` falls back to the tree walk.
Reached via `DomIndex::tag_bucket` / `DomIndex::has_foreign`; invalidated with
the attr index. **Every evaluate reads the index afresh from the handle**: a
reused `XPathContext` must never keep the one it first saw, because a mutation
frees it - that stale pointer was a use-after-free that answered from another
document's index (`spec/xpath_context_mutation_spec.rb`).

**text index** (`lexbor/adapter/text_index.rs`). Removes the per-call descendant
walk from text extraction (the cache-bound cost on Lexbor's 96-byte nodes). One
lazy build (count, size once, fill; explicit **heap**-stack DFS via
`grow_capacity` + `mkr_reserve_exact`, no recursion → no stack DoS) records a flat document-order
array of every TEXT/CDATA node's **borrowed** `BorrowedText` slice, a
prefix-sum of their lengths, and a `PtrTable` mapping
each element/fragment to the `[start,end)` run of slices its subtree owns. A
`Node#text` is then a hash lookup + `ruby_str_from_slices` (one pre-sized
memcpy run; **~4× faster than libxml2 at all sizes**), no element node touched.
Cached on the parse handle; `HtmlParsed::invalidate_indexes` drops it
from the **same single mutation hook** as the attr index, so a borrowed slice
can never point at reallocated/detached text storage. Reached via
`HtmlParsed::text_slices` (None → caller walks: fragments, build OOM).
Fail-closed: a build OOM leaves it unbuilt and the walk fallback serves.

**XPath engine** (`src/xpath/`). Original implementation: lexer →
recursive-descent parser → AST → evaluator + 26 built-in functions. The only
external hook is `Dom::qualified_name` (in `xpath/dom.rs`). Per-evaluate
budgets (op count, recursion depth, step/predicate/arg counts, node-set & string
caps) live in `xpath/limits.rs` and fail closed with `XP_ERR_LIMIT`. Ruby:
`Node#{xpath,at_xpath}(expr, handler=nil)`, `Makiri::XPathContext`
(`.new`, `#evaluate`, `#register_namespace`/`#register_ns`, `#register_variable`).
`#xpath` returns a NodeSet for node-sets, else String/Float/boolean. Errors map
SYNTAX→`XPath::SyntaxError`, LIMIT→`XPath::LimitExceeded`, else `Makiri::Error`.
Custom functions: unknown calls route through the engine resolver to
`handler.<local_name with - → _>`, run under `rb_protect` (a Ruby exception
becomes `Makiri::Error`, never a long-jump through the evaluator); node-set
returns from a foreign document are rejected. A handler may not modify the document
being evaluated: while an evaluation with a handler runs, every mutator on that
document raises `Makiri::Error` - the factories (`create_*`, `clone_node`,
`import_node`, `fragment`) included (`bridge::doc::DocumentEvaluation` /
`ensure_document_mutable`; XML writes reach the arena only through
`bridge::xml::arena_mut`, which checks it) - because the engine borrows names,
values and index slices across the walk, Lexbor frees an attribute's old value
on set, and an XML factory grows the very arena vectors the walk reads. The **namespace axis is not
implemented** (raises "not implemented", never silently empty); Nokogiri/libxml2
*does* implement it (e.g. `<svg>` in HTML yields the `xml`+`svg` namespace
nodes), so this is a documented behaviour difference - see
`NOKOGIRI_DIFFERENCES.md`. `namespace-uri()`/`local-name()` are implemented.
**Namespace matching of name tests is strict by default** (HTML5/WHATWG-faithful,
like browsers' `document.evaluate` and `Nokogiri::HTML5`): an *unprefixed*
element name test resolves in the HTML namespace, so `//div` matches but
`//svg`/`//path` do NOT - foreign (SVG/MathML) elements need a registered
prefix (`//svg:path`). Pass `namespace_matching: :lax` (on `Node#{xpath,at_xpath}`
or `XPathContext.new`) for the namespace-agnostic, `Nokogiri::HTML`-style match
where `//path` finds the SVG element. **The rule for the two modes: strict is
the specification, lax is Nokogiri** - whatever Nokogiri does for the host.
So the mode affects *only* unprefixed element name tests in HTML
(`Nokogiri::HTML` has no namespaces); prefixed tests, the `*` wildcard and
attribute tests are unchanged, and in XML the flag changes nothing, because
`Nokogiri::XML` (libxml2) is as namespace-strict as the spec. The decision is
the `Dom::unprefixed_matches(n, is_attr, lax)` policy item, which the axis and
the `[@a]` fast path both reach through `nodetest::unprefixed_attr_matches`. Makiri keeps HTML elements in the
XHTML namespace (so `namespace-uri()` is correct, unlike `Nokogiri::HTML5`'s
null). **Name tests fold ASCII case on HTML elements** (browsers + WPT
`domxpath`, NOT the HTML Standard, whose XPath section only sets the default
element namespace): `//DiV` finds `<div>`, `[@Id]` its `id`, in both modes;
SVG/MathML names stay exact (`refX`). One rule, `nodetest::names_equal` over
`Dom::folds_name_case`, serves the name test AND the `[@attr]` fast path - keep
it that way, since Lexbor's own attribute lookup folds on every element. The
HTML attribute axis also skips attributes in the XMLNS namespace (a foreign
element's `xmlns`/`xmlns:*`), as the XML backend skips declarations.

**Host policy lives in `Dom`, never in a host test.** Where XPath over HTML
and over XML differ, the difference is a named item of the `Dom` trait
(`xpath/dom.rs`, "host policy"): `test_name` / `attr_test_name` (local vs
qualified name), `unprefixed_matches` (the strict rule), `attr_ns_uri` (an
attribute's OWN namespace - `namespace-uri(//div/@id)` is `""`),
`ID_ATTRIBUTE` (`id` in HTML, none in XML), `LANG_ATTRIBUTES`. The engine
never asks which host it walks; the `IS_XML` flag that did is gone, and a new
policy is a new item stated in each `impl`. The HTML backend reports the DOM's
case-preserved `localName` (`refX`, `foreignObject`), not Lexbor's lower-cased
stored name.

**CSS** (`lexbor/selectors.rs`). `Node#{css,at_css,matches?}` via Lexbor's
`lxb_selectors`. The engine (`css_memory`+`css_parser`+`css_selectors` and the
`selectors` traversal object) is **built once and reused for every query** -
safe with no locking because CSS holds the GVL throughout (it never releases
it), so calls are serialized; between calls only the parsed list's arena is
reset (`lxb_css_memory_clean`) and the parser returned to its CLEAN stage
(`lxb_css_parser_clean`), and the traversal engine self-cleans after each
find/match. Per-call create/destroy used to dominate a cheap query and lost to
nokolexbor on `at_css('#id')`; reuse makes it ~5× faster than nokolexbor.
`lxb_selectors_find` runs with `MATCH_FIRST` to dedup comma lists; `at_css`
**stops at the first match and wraps that one node** (no NodeSet / no Ruby
`#first`). Results are **descendant-only** (context node excluded, like Nokogiri)
and in document order; capped at `NODE_SET_MAX`; malformed →
`Makiri::CSS::SyntaxError` (the shared engine is reset, so it recovers).
**The GVL is an argument, not a comment**: the two process-global engines live
in `crate::gvl::GvlCell`, whose `borrow` takes a `&Gvl` - minted from a
`magnus::Ruby` by `bridge::gvl::held`, and `!Send`, so `without_gvl` (which
requires a `Send` body) cannot carry one across a release. The cell's busy
flag turns a re-entrant second borrow into `Busy` rather than a second
`&mut`. Outside Ruby (cargo tests, fuzz) `Gvl::exclusive()` stands in with a
process-wide mutex; it does not exist in the extension build.
The parser/arena/table trio is assembled by `lexbor::css_engine`
(`ParserParts`, `Owned<T>`), which the selector-lowering parser
(`css_parser`) and the stylesheet reader share; the compiled-selector cache's
decision and storage are `CachePolicy` / `SelectorCache`, and the arena and the
map are only ever emptied together (`spec/css_selector_cache_spec.rb`).

**Serialization** (`lexbor/serialize.rs`). `Node#{to_html,to_s,outer_html}` =
Lexbor `serialize_tree_cb`, `#inner_html` = `serialize_deep_cb`; the callback
collects Lexbor's many small chunks into one growing C buffer (`cbuf::Buf`,
**pre-reserved to ~the output size** via `buf_reserve` so the per-chunk
appends don't realloc on every geometric step) and the
whole thing is copied into a UTF-8 Ruby String once - markedly faster than
`rb_str_cat` per chunk (its per-append capacity + coderange bookkeeping was the
serializer's dominant cost), and at parity with `nokolexbor`. (Serializing
straight into a growing Ruby String avoids the final copy but measured *slower* -
the intermediate growth is GC-tracked; the untracked C buffer + one copy wins.)
`pretty: true` uses `serialize_pretty_*` (Lexbor
quotes text nodes in that mode). A `DocumentFragment` serializes via the deep
serializer (the tree serializer rejects a fragment node). `Node#text`/`#content`
(`html_node::read::content`) serves descendant text from the **text index** (see
below) - a hash lookup + one pre-sized `ruby_str_from_slices` memcpy run,
no per-call tree walk - and falls back to a direct iterative walk for
non-indexed nodes (fragments). For a Document it returns the **root element's**
text (DOM makes a Document's textContent null, which is not what callers want).

**Mutation** (`glue/html_node/mutate.rs`). Tree edits (`add_child`/`<<`,
`add_previous_sibling`/`before`, `add_next_sibling`/`after`, `remove`/`unlink`,
`replace`) over Lexbor insert/remove. We **detach, never destroy** - the arena
owns node memory and live Ruby wrappers may alias a removed node; move semantics
= detach-then-insert. Attribute `[]=` / `delete`; `Node#name=` renames in place
(create a fresh element so the doc interns the name, copy its
`local_name`/`prefix`/`ns`/`upper_name`/`qualified_name`, destroy the throwaway -
identity preserved); `Node#content=`. `Document#{create_element,create_text_node}`.
Fragments: `DocumentFragment.parse(html)` (own backing doc) and
`Document#fragment(html)` (bound to a doc) parse in a throwaway `<body>` context
and `lxb_dom_document_import_node` (deep) each child into the target arena;
inserting a fragment splices its **children**. The four structural verbs are one
`bridge::html::insert(this, node, Place)`; every rule Lexbor omits (no parent,
no self-cycles, attribute nodes can't be tree children, doctype order) is the
adapter's `Insertion::check`, run before any link changes, and the placing is
`HtmlNodeMut::place`. **Every edit starts at `bridge::html::edit`, which drops
the indexes** (`HtmlParsed::invalidate_indexes`) - so no mutator calls it, and
none can forget to on an error path. `inner_html=`/`outer_html=` are all or
nothing: `stage_fragment_in` imports into a DETACHED fragment first, and only
then are the old nodes swapped out (`rake oom`'s `html_inner_html` scenario
checks the document is unchanged after every injected failure).

**Ruby surface niceties.** Node classes, under the WHATWG DOM interface names:
Document, Element, Attr, Text, Comment, CDATASection, ProcessingInstruction,
DocumentType, DocumentFragment (mapped by DOM node type in
`wrap_html_node` / `wrap_xml_node` - one per representation, since the
leaf classes are `Makiri::HTML::*` and `Makiri::XML::*`). `CDATA` and `DTD` are
Nokogiri-compatible aliases, defined in Ruby (`lib/makiri/compat_aliases.rb`) at
all three scopes; there is no `Attribute`. Convenience: `Node#{root,ancestors,path}`
(path round-trips through `#at_xpath`), `Node#{attributes,to_h}`,
`Node#{search,at}` (CSS/XPath auto-detect: starts `/ ./ .. .// ( @` or contains
`::` ⇒ XPath, else CSS), `Document#{body,head,encoding,meta_encoding}`
(`encoding` is always "UTF-8"). `NodeSet#{|,+,&,-}` are identity-dedup,
encounter-order (**not** doc-order), `#{css,xpath,search}` run per node and union
(return `self` when empty so an empty set stays a NodeSet), `#{last,at,remove}`.
`Element.new(name, doc)` / `Text.new(content, doc)` delegate to
`Document#create_*` (they override `.new`, since the allocator is undef'd).

## Performance

**Makiri beats Nokogiri/libxml2 on every `rake bench` row.** Measured
against Nokogiri: parse ~4.6×, css ~12×, at_css ~9400×, `//tag` ~4×,
`//*[@id=…]` ~8×, `[@attr='v']` ~4.3×, attribute axis ~3×, serialize ~6×,
full-text extraction ~3.5×. **traverse** (children walk) used to be the one row
that only met Nokogiri (within measurement error); as of the v0.10.0 bench it
beats it too.

Treat these as indicative, not precise. Two consecutive runs on the same machine
put full-text extraction at 2.9× and 3.5×, and threaded parse scaling at 2.4×
and 1.7×; benchmark-ips reports ±18-27% on the heavier rows. What the numbers
are good for is catching a *regression in kind* (a row falling to parity or
below), not for defending a second decimal place.

Parsing also scales across threads (~1.7-2.4× on 8 cores) because it releases
the GVL; XPath does not scale, by design (it holds the GVL - see below).

Key decisions that got there, worth not regressing:

- **Parsing releases the GVL; XPath evaluation does NOT** (`glue/doc.rs`,
  `glue/xpath.rs`): parse copies the source to a C buffer then runs
  `parse_html` under `rb_thread_call_without_gvl` - safe because a freshly
  parsed document is not yet shared, so it can't race anything. **XPath holds
  the GVL for the whole evaluation by design** (`xpath::ctx::Context::evaluate` is a plain
  GVL-held call). The engine and DOM are not thread-safe against concurrent
  mutation, and holding the GVL makes that safe *by construction*: the GVL
  serialises all Ruby-thread C code, so an XPath walk never runs in parallel
  with a tree mutation, with another `evaluate` on the same context, or with a
  `register_variable`/`register_namespace`/`node=` on the same context - no
  locking needed (and none is used). An earlier version released the GVL for
  handler-free XPath (it scaled queries ~3.3× across threads), but the locking
  required to make a GVL-released walk safe against shared-document mutation was
  judged not worth the verification burden; single-thread XPath speed is
  unaffected by holding the GVL. **Do not reintroduce a GVL-released XPath
  path** without the full document/context locking story. Parse still scales
  (~2× on 8 cores); verify with `bench`'s threaded rows and the `GC.stress`
  `spec/threading_spec.rb` (which now also asserts shared-document XPath+mutation
  and shared-context evaluate are crash-free under the GVL).
- **`//tag` is served from the element index** (`xpath/step_index.rs`): a document-rooted, predicate-free, unprefixed
  descendant name-test pushes the tag bucket instead of walking. Pure-HTML only
  (`has_foreign` guard) and each candidate is re-checked with the step's
  `CompiledTest`, so the result is identical to the walk; custom/unknown
  tag names fall through. See the element index note above.

- **The CSS engine is built once and reused** (`lexbor/selectors.rs`, see the
  subsystem note): the per-call create/init/destroy of the Lexbor CSS object
  graph dominated a cheap query and lost to nokolexbor on `at_css('#id')`; a
  process-global engine (safe because CSS holds the GVL throughout) reset with
  `lxb_css_memory_clean` + `lxb_css_parser_clean` between calls makes `at_css`
  ~6000× Nokogiri / ~5× nokolexbor (was ~1.16× *slower* than nokolexbor). `at_css`
  also wraps the single first match directly (no NodeSet / no Ruby `#first`). Do
  not reintroduce per-call engine teardown; verify with `bench`'s `at_css`/`css`
  rows and `fuzz:sanitize --target css` (the reuse is the memory-safety risk).
- **`Node#text` is served from the text index** (`lexbor/adapter/text_index.rs`,
  see the subsystem note): a per-document, lazily-built, mutation-invalidated
  map from node → its document-order text-slice run, turning text extraction
  into a hash lookup + one pre-sized memcpy instead of a cache-bound walk over
  96-byte nodes (~4× libxml2, vs the former ~parity). The walk fallback stays
  for non-indexed nodes. Do not regress to walking on the indexed path; verify
  with `bench`'s "full document text" row and `spec/text_index_spec.rb` (which
  asserts byte-identity with a plain walk across subtrees + mutations).
- **String-value cache is hashed** (`xpath/str_cache.rs`): a token-keyed
  `PtrMap` index over an ordered store, so per-node predicate compares
  are O(1), not the old O(n²) linear scan. The cache belongs to one evaluate
  (`xpath::eval::Evaluation`), as do the op budget and the document-order
  index, so a nested (handler-triggered) evaluate gets its own and cannot
  disturb the walk that called it.
- **`[@name]` / `[@name='lit']` predicates take a direct-attribute fast path**
  (`match_attr_pred`/`attr_pred_matches` in `xpath/attr_pred.rs`): a
  position-independent filter via `lxb_dom_element_has_attribute`/`get_attribute`
  instead of building a throwaway node-set per candidate; anything else falls
  through to the generic evaluator.
- **`Node#at_xpath` first-match short-circuit** (`xpath/eval.rs`
  `try_first_match`, entered via `evaluate_first`): `at_xpath`
  wants only node-set[0], so for the common "first descendant by name (+ a
  position-independent `[@a]`/`[@a='v']` predicate)" shapes - `//x`, `//x[@a]`,
  `//*[@a='v']`, `.//x`, `descendant::x[...]` (after the `//` peephole; one or two
  steps) - it walks the subtree in **document order and stops at the first match**
  instead of materialising+sorting the whole set, the XPath analogue of at_css's
  `MATCH_FIRST`. Cost becomes O(position of first match): a front hit is ~µs
  (vs ~280µs full-eval), trailing/absent fall back to a full scan. Reuses
  the step's `CompiledTest` + the `[@attr]` matcher so it's **byte-identical to
  `xpath(e).first`** (asserted by `spec/at_xpath_first_spec.rb`). Anything else
  (positional predicates, functions/variables, reverse axes, unions, prefixes,
  longer paths) returns 0 from the recogniser → full evaluator. Only `at_xpath`
  uses it; `xpath` always builds the full set.
- **Per-context compiled-AST cache** (`glue/xpath.rs`): an `XPathContext` parses
  each expression once and re-runs the cached AST (bounded by `AST_CACHE_MAX`).
  `Node#xpath` uses a throwaway context and does not cache.
- **Every Document reports its arena to the GC** (`account_document`, which
  `DocumentShell::install` calls; `DocData::release` takes the report back). Neither
  Lexbor's pools nor the XML arena is an `xmalloc`, so without the report Ruby
  sees a parsed Document as ~56 bytes and NO collection is triggered by memory
  pressure: `500.times { Makiri::HTML(html) }` ran with zero GCs, 2.2 GB RSS,
  and every parse faulting fresh pages - the parse bench read 1.8× slower than
  nokolexbor at ±43% variance, and nothing failed. With the report it is at
  parity. The diagnostic is `GC.count` across a parse loop (must rise) and
  `minflt` per parse (near 0 once warm). A Document gets its arena ONLY through
  `bridge::wrapper::DocumentShell`: the wrapper is allocated first (a Ruby
  allocation can raise, and a raise would leak a parse result already held),
  and `install_html(Box<HtmlParsed>)` / `install_xml(Box<XmlDoc>)` store the
  content and report it in one step, so a
  new parse entry cannot skip the report; `spec/gc_accounting_spec.rb` pins
  both halves. Growth through mutation/fragment import is NOT re-reported
  (an approximation, in the safe direction of under-reporting).
- Tree-walk speed is structurally capped by Lexbor's 96-byte node (we can't
  shrink it); investigated nodeset-pool / prefetch follow-ups were **not** shipped
  because, with no remaining slower-than-Nokogiri row, they'd add lifetime /
  speculative complexity for no measurable win.

Working on perf: capture a `rake bench` baseline, change one thing, re-bench,
and **ship only a measurable win** that keeps `rake spec` + `rake fuzz:sanitize`
green. (Note: a per-node Ruby wrapper cache was considered and rejected -
`node->user` is taken, and a node→VALUE side table creates a GC-lifetime problem
for no clear gain on already-winning paths.)
