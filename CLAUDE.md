# Makiri - project guide for Claude Code

Makiri is a Ruby gem: an HTML5 parser + native XPath 1.0 query engine + CSS
selectors, with **no libxml2 / libxslt dependency at any layer**. It parses with
a Rust port of Lexbor's HTML parser into an index-arena DOM (vendored Lexbor
remains as the reference it is tested against) and queries via an original
XPath engine. Security is a
first-class goal.

The authoritative design is **`docs/design_doc.ja.md`** (Japanese) - read it
before architectural decisions. This file is the operational summary: constraints,
how to build/test, the subsystem map, and the non-obvious gotchas. The exhaustive
API list lives in the code + specs + `CHANGELOG.md`, not here.

## Hard constraints

- **Vanilla Lexbor, no fork, no patches.** `vendor/lexbor` is a git submodule
  pinned to a release **tag**. Never `git apply` to it. It is the reference the
  ports are tested against, and where a port departs from it - a Lexbor gap, or
  the Standard read differently - the port says so
  (`docs/parser_differences.ja.md`, `compat`), never an edit to Lexbor.
- **No libxml2 / libxslt** anywhere - not linked, vendored, or derived. The
  XPath engine is original. See `NOTICE`.
- **One language.** The extension is a single Rust crate
  (`ext/makiri/rust`), and no C ships at all: the vendored Lexbor is built
  only for the tests (`rake lexbor`) and keeps its `lxb_*` names. No SYMBOL of ours carries a C-style prefix any more: the crate
  exports only `Init_makiri` and `ruby_abi_version`, so every other item is
  named as Rust. (The `mkr_` prefix was the C ABI's symbol convention and went
  with it.) The ONE surviving prefix is `falloc`'s extension methods
  (`falloc_reserve` / `_reserve_exact` / `falloc_push` / `falloc_insert`), and
  it cannot be dropped: `Vec::try_reserve` and
  `try_reserve_exact` are inherent std methods, so a trait method by either
  name would be silently shadowed by the ABORTING one - which is the bug
  `falloc` exists to prevent. It was `mkr_` and now names the policy instead.
- **Security-first / fail-closed.** Enforce per-evaluate XPath budgets and
  node-set caps, validate inputs, never return a truncated/wrong result (raise
  instead). **Export only `Init_makiri`** (plus `ruby_abi_version`, which Ruby
  reads at require time). The extension links no Lexbor any more, but it did,
  and a bundle that re-exported its ~1700 `lxb_*`/`lexbor_*` symbols let
  another Lexbor-based gem in the same process (e.g. `nokolexbor`) bind its
  `lxb_*` calls to our different Lexbor version → segfault; any name of ours in
  the dynamic table is the same hazard. **Verify with `bundle exec rake symbols`**, which asserts the whole
  claim (nothing but those two names exported). Do not weaken that gate to "no `lxb_` exported" - it passed
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
  after the work - a cleanup call following the work is exactly what
  unwinding skips. (The two Lexbor-era cases, `lexbor::selectors::Session` and
  `post_parse::DocOwner`, are reference-only now.)

  **A callback must not panic INTO C.** Rust turns an unwind at an `extern "C"`
  boundary into an abort, because unwinding through frames built without unwind
  tables is undefined behaviour - so the callback catches the panic, latches it
  and returns the stop status, and the caller re-raises once C has unwound.
  `caught::PanicLatch` is that, and it is deliberately the same shape the
  callbacks already used for the node cap and OOM. The build that ships
  installs it in two: `bridge::gvl`'s trampoline (which carries the whole
  parser) and `glue::hash::hash_foreach`. For the same reason a Hash is walked only through
  `hash_foreach` (magnus's `RHash::foreach` protects without one;
  `rake unsafe:boundaries` fails on a direct `.foreach(`). The reference-only
  Lexbor callbacks carry it too (`lexbor::selectors`' traversal,
  `lexbor::chunks::chunk_cb`, `tree_guard::hook_token_cb`).
  Two `extern "C"` functions have no latch ON PURPOSE. The GC callbacks in
  `bridge/typed.rs` run where nothing can be raised and a half-freed object is
  worse than a stop, so they keep aborting - keep their bodies trivial. The
  `rb_protect` thunk of `bridge::raw::call` holds one C call and the `Copy`
  values it reads (`F: Copy`, `T: Copy`), so there is no Rust there to panic
  and no `catch_unwind` for a raise to cross; keep it that way.

  **A VALUE inside a wrapped object is a `bridge::typed::Held`**, and a
  `Held` that holds anything can only be made with a `Store`, which exists
  only where the owner is already a Ruby object (`TypedType::wrap`'s store
  step, `TypedType::with_store`). So "store a VALUE into a struct only after
  its wrapper exists" - the `DocData.errors` crash - is a type rule, not a
  convention: a struct is built holding `Held::empty()`, and its
  `Hooks::trace` hands each `Held` it keeps to the `Tracer`, as
  `pinned` or `movable` - one walk for both the mark and the compaction
  callback, so a value marked movable cannot be left out of the relocation.
  The type enforces that ORDER only: that a `Held` stays where its owner's
  trace finds it is beyond the type system, so reading one back
  (`Held::get`) is `unsafe` with that as its contract, and both types are
  visible to `bridge/` alone. A `Held` is a `Cell`, and a `DocData`'s
  changing fields are cells too, so the GC callbacks and every method read
  a wrapper's data through `&T`: no `&mut` of it is made at all.
  The other half - no magnus value kept where the GC's stack scan cannot
  see it (a `Vec`, a `Box`, a static or `thread_local!`, a `TypedData` /
  `Hooks` type's own field) - is `rake dylint`'s `ruby_value_on_heap`,
  which reads the types; it is why raw `VALUE`s stay in `bridge/raw.rs` and
  `bridge/typed.rs`, since a lint sees a `VALUE` only as an integer. What it
  cannot see is a temporary handed straight to a call (`f(vec![v])`).
  A node wrapper's `NodeData` is EMBEDDED in its object's slot
  (`TypedType::embedded_base` / `embedded_derived`, `RUBY_TYPED_EMBEDDABLE`,
  Ruby 3.3+): one allocation per wrapper, and the free callback drops the
  data without freeing memory. Compaction moves that data with the object,
  so a `&NodeData` must not be held across anything that can allocate -
  every reader copies the handle and the Document out at once, and a new
  embedded type must keep to that. (The NodeSet was not embedded for this
  reason: its fill holds `&NodeSet` across its own growth.)

  **Every C call that can raise is a wrapper in `bridge/raw.rs`, and every
  wrapper runs it under `rb_protect` and returns `Result`.** A raise is a
  `longjmp` over every Rust frame up to the nearest `rb_protect`, and crossing
  a frame with a `Drop` or a `catch_unwind` is undefined behaviour; so each
  wrapper hands its one C call to `raw::call`, whose `extern "C"` thunk is the
  only frame a raise crosses, and the raise comes back as `Err`. A caller may
  then own copies, hold document guards and build Ruby objects in any order.
  `rake unsafe:boundaries` pins the raising names to `raw.rs` and fails on one
  outside a `call(|| ...)` there (`raw_unprotected`); a new raising call is a
  new wrapper with its raise stated. magnus's allocating constructors raise
  instead, so `clippy.toml` bans them (and the `funcall`/`respond_to`/`eql`
  family, which runs the caller's Ruby): the glue builds Arrays, Hashes,
  numbers and Symbols through `bridge::build`, Strings through
  `bridge::string::utf8_str`, and NodeSets through `typed::wrap_magnus`. Only
  `Init_makiri` (where a raise is `require`'s) carries an `#[allow]`. Each
  protected call costs a `setjmp`, ~8 ns: a light reader that allocates one
  String (`#name`, `#[]`, `#text`) measured ~15-20 ns slower than the
  unprotected allocation it replaced, about half of which dropping the
  lifting (below) took back (`notes/ruby_boundary_simplification_plan.ja.md`).
  The value-level wrappers are SAFE and typed: they take and return magnus's
  values (`RString`, `RArray`, `RHash`, `Value`), slices and
  `bridge::string::Encoding`, check what the C function would not (a Hash
  key's kind, a substring's bounds, `to_encoding`'s argument), and one with
  no Ruby value takes `&Ruby` as the GVL's proof. Only those over raw memory
  stay `unsafe` (`typed_object_wrap` / `_zalloc`, `xmalloc`, `xrealloc2`,
  `check_typeddata`, `utf8_str_unwritten`). A raw `VALUE` - an integer to
  Rust, which no lint can tell from a `u64` in a `Vec` - is spelled only in
  `raw.rs` and `typed.rs` (`unsafe:boundaries`' `VALUE_COUNTS`, empty), so
  everything else holds typed values; `TypedType::wrap` is safe for that
  reason (`&Ruby`, and a `T` can keep a Ruby value only in a `Held`). A
  `Held` is a private field beside the `trace` that marks it, read through
  that module's accessor (`NodeData::document`, `DocData::errors`).

  **A Document's content is reached only through a guard** (`bridge::dom`,
  over `crate::doc_cell`, which is Ruby-free so its `compile_fail` doctests
  pin that a guard's loan cannot outlive it and that a guard stays on its
  thread): the `DocData` owns it in a `DocCell`, and a read (`HtmlRead` / `XmlRead`,
  or the receiver views `HtmlSelf::read` / `XmlSelf::read`) or an edit
  (`HtmlWrite` / `XmlWrite`, taken only inside `HtmlEdit::with_tree`,
  `new_node`, `Editing::with_arena` and their kin) is a counted borrow. No
  raw pointer to the content is kept or handed out, and none of its old
  `unsafe` contract ("nothing edits it while you hold this") is left to a
  call site: an edit that begins while any read is held - a copy of the
  receiver - is refused as `Makiri::Error`, and a read during an edit is an
  `InternalError`. An edit that changes two documents
  (an adoption: the copy placed, the original released) takes both writes
  before it changes either, so it is refused whole or done whole. A guard is a `Drop`, and a raise past
  one would leave the document borrowed for good; no raise crosses one,
  since every raising call returns `Err` (above). `rake oom`'s raise sweep
  makes every allocating call raise in turn and checks no document is left
  borrowed. So a reader builds its result with the read held
  (`str_from_read`, `node_set_from_read`); the copy-out-then-build it used to
  need ("lifting") went with stage 4 of
  `notes/ruby_boundary_simplification_plan.ja.md`, and the light readers got
  faster for it (`#text` ~124 -> ~112 ns).

  **A Ruby method raises `Makiri::InternalError`, not `fatal`.**
  `bridge::ruby::entry` wraps the body of EVERY method the glue registers, and
  turns a panic there into that exception. It used to wrap only the methods
  judged to reach untrusted input, and the judgement left readers such as
  `children`, `[]` and `NodeSet#each` out; so `rake unsafe:boundaries` finds
  every `method!`/`function!` in the glue (a registration table included),
  resolves it to its one definition, and fails unless the whole body is a
  `crate::bridge::ruby::entry(...)` call. `ENTRY_EXEMPT` there names the few
  that stay out - the `__panic` / `__alloc_inject*` test hooks - and a new
  exemption needs a reason.
  `InternalError` descends from `Exception`, NOT `StandardError`, which is the
  point: a bare `rescue => e` keeps passing it through, because a broken
  invariant is not a bad selector, while a host that wants to turn one request
  into a 500 can catch it WITHOUT a thread boundary to re-raise at (a `fatal`
  cannot be rescued in its own frame at all). Outside a registered method - the
  exempt ones, GC callbacks, init - a panic stays `fatal`.

  `clippy::unwrap_used`, `clippy::expect_used` and `clippy::panic` (in `Cargo.toml`) keep a new panic
  from arriving by accident; a site that wants one carries an `#[allow]` with a
  reason. `spec/panic_spec.rb` drives `Makiri.__panic(kind)`: kind 4 panics
  below the GVL-release frame, the case that proves the latch since without it
  that one aborts, kind 5 goes through `entry`, and kind 6 panics in the
  token-done path of a real document parse (`html::tree`'s
  `parse_with_panicking_hook`, its own sink, so no real parse carries it).

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

Pinned to **`05b5d37`** (`v3.0.0-66-g05b5d37`, builds cleanly with our
`LEXBOR_BUILD_SHARED=OFF` config). **Normally we pin to a release tag**, but this
is an *untagged master* commit taken deliberately, and **v3.0.1 is NOT usable**:
it is a maintenance release off v3.0.0 and does not contain the **two
CSS-selector fixes we upstreamed** - `#369` (`3a2d595`: class/ID selectors match
case-sensitively except in quirks mode, like browsers) and `#371` (`940162d`: a
prefix-less type selector no longer defaults to the universal namespace) - so
moving to it would REGRESS matching. Check that again on the next tag.

Master also carries what Makiri needs: the **heap-overflow fix in the
`:lexbor-contains()` parser** (reached via `Node#css`), the `#365` tokenizer
size-limit (DoS) fix, and - new in this pin - an **out-of-bounds write in
`lxb_dom_character_data_replace`** (`cf31487`), which `Node#content=` on a
text/comment/PI node reaches through `lxb_dom_node_text_content_set`, plus
UTF-16 surrogate rejection, exact `meta` charset matching, buffer-capacity fixes
in the integer/hex conversion, and selector-destroy leaks.

**This pin changes parsing**, which no earlier bump did: `f441451` implements the
HTML Standard's new **processing-instruction tokens**, so `<?php ...?>` in HTML
input is a `ProcessingInstruction` node where it used to be a Comment, `#to_html`
writes `<?target data?>`, and `<?` at EOF is ignored (the spec's
"eof-in-processing-instruction"). `Nokogiri::HTML5` still answers the old way -
see NOKOGIRI_DIFFERENCES.md. The differential baseline's 81 affected lines were
edited with the bump. `spec/conformance/html5lib_runner.rb` reads its
tree-construction data from WPT (`html/syntax/parsing/resources`, where the
html5lib .dat files have lived since 2026-06), which carries the new
expectations; what this Lexbor pin still answers the old way is listed in
`KNOWN_DIVERGENCES` there and in `docs/parser_differences.ja.md`.

Still **vanilla, NEVER patched** - the constraint that relaxed is "release tag
only", not "no fork".

**`:lexbor-contains()` is not supported, and `lexbor::contains_guard` keeps
every one from the vendored CSS parser - it is not optional.** Its argument
parser is where the v3.0.0 heap overflow was, and a failed argument parse left
a pseudo-class the serializer crashed on; deciding which arguments were safe
kept failing, so the guard renames EVERY `lexbor-contains(` function, whatever
its argument, and the parser rejects it as an unknown pseudo-class (a selector
is a `CSS::SyntaxError`, a stylesheet rule `bad_style`). Its module doc states
the two properties that must hold and why; `lexbor/tests.rs` pins them against
the real parser. It finds the name on the tokens of Lexbor's OWN CSS
syntax tokenizer (`lexbor::css_tokens`), never on a scanner of ours: the hand
scanner it replaced ended strings where the tokenizer did not (CR, FF,
backslash-newline), so a `:lexbor-contains(#x)` after such a string reached the
parser unseen and `parse_stylesheet` crashed serializing what it left behind.
**Since the parser port no build that ships runs Lexbor's CSS parser at all**:
selectors and stylesheets are both parsed by `crate::selectors`, which leaves
`lexbor-contains` out of its function table and so rejects one as an unknown
function - the answer the guard gave. The guard, `css_tokens`, `css_engine`
and `css_parser` are compiled only into the tests and the reference features,
where Lexbor's parser is the oracle. Both reference call sites still go
through the guard - `css_engine::parse` (selectors) and
`stylesheet_diff::lexbor_dump` (stylesheets) - and a new one must:
`rake unsafe:boundaries` allows each Lexbor CSS parse function exactly one
call site, the guarded one. The guard and its tests come out together or not
at all, once no reference runs Lexbor's parser.

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

```bash
git submodule update --init        # fresh clone only; the reference Lexbor
bundle install
bundle exec rake compile           # the crate alone - the extension links no Lexbor
bundle exec rake lexbor            # the reference Lexbor (script/build_lexbor.rb,
                                   # cmake); lint, rust:test,
                                   # conformance:parser and the diff fuzzers need it
                                   # and depend on it
bundle exec rake spec
bundle exec rake rust:test         # Ruby-free core tests, Lexbor-feature tests, fuzz-crate check
bundle exec rake lint              # cargo clippy --all-features -D warnings + cargo fmt --check
bundle exec rake unsafe:boundaries # reviewed unsafe islands and safe-module boundaries
bundle exec rake dylint            # the crate's own lints (lints/makiri_lints):
                                   # ruby_value_on_heap - a magnus value in a heap
                                   # container, a static/thread_local or a wrapped
                                   # object's data; converted_argument - a registered
                                   # method taking a type magnus converts (RString,
                                   # i64, ...); then their self-test
                                   # (src/lint_probe.rs). Needs the nightly
                                   # rust-toolchain.toml pins + cargo-dylint/dylint-link
bundle exec rake clean             # wipe the build dir (regenerates the Makefile next compile)
bundle exec rake clean:lexbor      # wipe vendor/lexbor/{build,dist} (full Lexbor rebuild)
bundle exec ruby -Ilib -r makiri -e 'p Makiri::VERSION'   # smoke load

bundle exec rake symbols           # THE export gate - see Hard constraints
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
                                   # so this and Linux Valgrind (`spec:valgrind`, any
                                   # "definitely lost" through the ext; CI runs it
                                   # nightly) are the leak
                                   # checks; flags per-call leak stacks through the
                                   # ext incl. rescued raises)
bundle exec rake oom               # OOM-injection sweep: rebuilds with
                                   # MAKIRI_ALLOC_INJECT=1 and fails each core alloc
                                   # site in turn - every OOM branch must fail closed
                                   # (clean raise or baseline-identical result), then
                                   # reuses the same document/context after GC.compact.
                                   # Then the raise sweep (script/check_raise_injection.rb):
                                   # each allocating C call of bridge/raw.rs raises
                                   # NoMemoryError in turn, and no document may be left
                                   # borrowed (`Makiri.__borrowed?`) or unusable - the
                                   # runtime check of the guards-across-raises rule.
                                   # Prefix with MAKIRI_SANITIZE=address to combine ASan.
bundle exec rake kani              # Kani proofs over the Ruby-free core (needs cargo-kani).
                                   # The successor to the C-era CBMC harnesses: the
                                   # allocator, cbuf, UTF-8 validate/decode.
                                   # KANI_GROUPS=alloc,utf8,xml,xpath picks groups
                                   # (Rakefile's KANI_GROUPS, with the code each
                                   # depends on); CI runs only the groups whose
                                   # code changed (`rake kani:groups`), and all of
                                   # them weekly. KANI_JOBS=N runs N in parallel.
bundle exec rake bench             # perf vs Nokogiri (bench-only gems; runs outside bundle)
```

Requires CRuby >= 3.3 and a **Rust toolchain** (`cargo`); `cmake` only for the
reference Lexbor (`rake lexbor`). The gem carries none of
Lexbor's sources, only its LICENSE and NOTICE, which the ports owe it. A source
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
  times - see `docs/ci-crash/INVESTIGATION.md`). Heap red zones are
  unaffected; only stack-buffer checks are lost.
  Do not re-enable without solving the `__builtin_longjmp` shadow problem.

  What used to soften this loss no longer applies and was not replaced:
  `-fstack-protector-strong` covered stack smashing in the C, and there is no C.
  Rust's own bounds checking is what covers the class now on every path that is
  not `unsafe`, which is why `unsafe` blocks carry a stated contract.
- **The XML arena (`src/xml/arena.rs`) needs no ASan poisoning, and has none.**
  The C build's bump arena did (poisoned 64 KiB chunks, the Lexbor-mraw blind
  spot); the Rust one is an index arena - a `Vec<Node>` and a `Vec<u8>` under
  `#![forbid(unsafe_code)]`, addressed by `NodeId` and spans - so a write past
  one node or byte span is a bounds-checked index, not an overrun into the
  next cut. Everything else we write allocates through `falloc` onto the system
  allocator, or - for the glue's Ruby-side storage - through Ruby's xmalloc;
  ASan red-zones both per allocation.
- **The fallible-allocation line.** The engine (`html`, `xml`, `xpath`, `css`,
  `selectors`, `css_match`, `css_cache`, `cbuf`) allocates only through `falloc`: `clippy.toml` bans the
  infallible `Box::new` / `Vec::with_capacity` / `reserve`, and `rake oom` fails
  each site in turn, so an OOM there raises instead of aborting. The glue's
  Ruby-side storage - TypedData wrappers (`bridge::typed::wrap_built`, freed by
  running the struct's `Drop` in place before `ruby_xfree`) and
  `NodeSet`'s node array - uses Ruby's `ruby_xmalloc` family instead: its failure
  is `NoMemoryError`, Ruby's own, which `bridge::raw`'s protected calls hand
  back as `Err` (a failed wrap drops the built struct and frees its memory).
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
  extconf.rb               hands the crate to cargo via create_rust_makefile; owns
                           the link arguments and the export trim (it builds no
                           Lexbor: `rake lexbor` does, for the tests)
  build.rs                 bindgen over Lexbor's own headers -> the layout and
                           constants the crate reads (never transcribed by hand)
  src/
    init.rs                Init_makiri: the class hierarchy and the registration seam
    falloc/                fallible allocation - every engine allocation goes
                           through here, so `rake oom` can fail it and OOM raises
                           rather than aborting the host process (the glue's
                           Ruby-side storage is Ruby's xmalloc; see the gotchas).
                           Its `try_box` holds the ONE `unsafe` block left in
                           the shipping crate outside `bridge/` (a fallible
                           `Box<T>` has no stable safe spelling); everything
                           else Ruby-free is `forbid(unsafe_code)`
    doc_cell.rs            `DocCell`: a Document's content behind counted
                           read/write guards (Ruby-free; `bridge::dom` wraps it)
    cbuf.rs                `Buf`: the owned, capped, growable byte buffer
    cutf8.rs               the one UTF-8 validator + strict 1-codepoint decoder
    utf8_input.rs          HTML input sanitiser: invalid UTF-8 -> U+FFFD, per
                           WHATWG; `cbuf`+`cutf8` only, so Ruby/Lexbor-free
    bridge/                the Ruby boundary - the ONLY layer allowed raw Ruby String
                           access (RSTRING) and verified-string minting, and where
                           the C calls that can raise (`bridge/raw.rs`) and the
                           wrap-then-store constructor live; a raise becomes `Err`
                           there. `rake unsafe:boundaries` pins that boundary:
                           the crate denies `unsafe_code` (`lib.rs`), so every
                           file that needs it carries an `allow` and the script
                           holds each one's count exactly - a new file fails even
                           where a parent module's `allow` kept rustc quiet - plus
                           every `forbid` file, which it counts. `glue/**`, `xpath/**`,
                           `css/**` and `css_match/**` are fully safe
                           (their module roots carry
                           `#![forbid(unsafe_code)]`, which the gate pins), and
                           `rb_sys::` and `Value::from_raw` are 0 outside
                           `bridge/`, and the C calls that can raise live only
                           in `bridge/raw.rs`
    glue/                  THE Ruby surface: every method Makiri defines, with
                           its argument reading, its errors and its
                           registration, one module per feature
                           (query/html_doc/xml_doc/node_set/xpath_context/
                           html_node/xml_node, each node module owning its
                           read/mutate/css/serialize). All `unsafe`-free; the
                           wrappers, TypedData and the operations that keep a
                           structure's invariants are `bridge/`'s, and
                           `rake unsafe:boundaries` fails on a method defined -
                           or an argument list scanned - in `bridge/`
    xpath/                 native XPath 1.0 engine, generic over a `Dom` trait;
                           `#![forbid(unsafe_code)]` and Lexbor/Ruby-free. The
                           two `Dom` instances live with their layers:
                           `html/dom/xpath.rs` (HTML) and `xml/xpath.rs` (XML),
                           joined for the glue by `Cx` in
                           `bridge/xpath/mod.rs`
    xml/                   native XML reader (Ruby/Lexbor-free; own arena), plus
                           its XPath `Dom` instance. One job per module:
                           `model`+`arena` (the index arena), `tree/` (the
                           builder, with `tree/cursor.rs` - the shared input
                           cursor - and `tree/dtd.rs`, the subset validator that
                           holds a cursor and so CANNOT reach the tree),
                           `chars/` (character classes + `chars/expand.rs`, the
                           reference-expansion engine), `qname` (XML naming) vs
                           `dom_name` (the WHATWG DOM's looser names, which are
                           NOT XML naming), `mutate`, `index`, `encoding_sniff`,
                           and `serialize/` (`out` = buffer + one escape table,
                           `xml` = plans a prefix per name, `c14n` = renders the
                           document's own declarations, adding a name's own
                           prefix where its namespace was set, never inventing one)
    html/                  the HTML parser ported from Lexbor and the DOM it builds
                           (Ruby/Lexbor-free): `tokenizer/`, `tree/` (the tree
                           builder, its guard, stamps and the document-bytes
                           parse), `dom/` (the index arena: handles, edits,
                           copies, indexes, serialization, CSS and XPath
                           instances), `encoding/` (sniffing, decoders),
                           `fragment.rs`, and `res/` (tables generated from
                           Lexbor's)
    lexbor/                the Lexbor boundary: `abi` - the generated layout and
                           functions (the `_noi` twins included), the ONE place
                           a Lexbor function is declared (a second `extern "C"`
                           spelling is a second Rust type for the symbol;
                           `rake unsafe:boundaries` fails on one). Only the four
                           exports no header declares are written by hand, and
                           build.rs's `UNDECLARED_EXPORTS` fails the build if
                           their C definitions change - `adapter`, the one reader of
                           Lexbor's DOM structs, plus the element index,
                           text index, source location and post-parse - and the
                           selectors/serialize/fragment facades, the views over
                           Lexbor's CSS selector lists (`css_parser.rs`; its
                           parse is a reference) and the XPath HTML backend
                           (`xpath.rs`). All of it is the reference the ports
                           are tested against: nothing here ships, and every
                           `lxb_*`/`Lxb*` name outside it is 0
    css_match/             the CSS matcher, over any DOM that implements its
                           traits (`css_match::dom`: the HTML arena's in
                           `html/dom/css.rs`, the XML arena's in `xml/css.rs`);
                           `#![forbid(unsafe_code)]`, Ruby- and Lexbor-free,
                           and naming no host. The compiled-selector cache in
                           front of it, which names both, is `css_cache.rs`
    css/                   the CSS-to-XPath lowering XML's CSS used to run on,
                           kept only as the reference the XML matcher is held
                           to (`xml::css_tests`, the `css` fuzz target; built
                           under `cfg(test)` or `xml-css-reference`)
    selectors/             the CSS parser ported from Lexbor (its CSS syntax
                           tokenizer, rule/state stacks, selector states, and
                           the stylesheet rule phases and callbacks in
                           `parser/syntax.rs`) into an owned AST and rule arena
                           (`rules.rs`), plus `parse_stylesheet`'s reader
                           (`stylesheet.rs`) and the CSSOM selector writer
                           (`text.rs`); Ruby- and Lexbor-free,
                           `#![forbid(unsafe_code)]` at its root
  fuzz/                    cargo-fuzz harnesses (xml/html, xpath/xml_xpath/
                           html_xpath, css = the XML matcher held to the lowering, html_css = the
                           HTML matcher over an arbitrary document, asserting
                           css and matches? agree, html_css_diff = the same
                           matcher against Lexbor's own engine, html_diff =
                           the ported tree builder against Lexbor's tree,
                           documents and fragments - with their
                           serializations and the depth guard's verdicts -
                           selector_diff /
                           stylesheet_diff = the ported selector / stylesheet
                           parser against Lexbor's; built on
                           PRs, run nightly. css-match.yml owns the two
                           html_css*)
vendor/lexbor/             git submodule, pinned 05b5d37 (v3.0.0-66), NEVER patched
spec/fuzz/                 grammar-aware robustness fuzzer
spec/invariants/           randomized property checks (see its README)
spec/differential/         the recorded C-build answers + the probes (see `rake diff`)
bench/                     Nokogiri-comparison benchmark
docs/design_doc.ja.md      authoritative design (read this)
```

Seven features, and the default is the extension: **`ruby`** (the
magnus boundary + `glue` + `init`), **`html-dom`** (the parser port's
index-arena DOM, `html::dom`, behind the `html_dom` facade and parsed by the
ported tree builder `html::tree`, with what sits over it - the HTML CSS
matcher, XPath's HTML instance, the bridge's HTML half; `ruby` and `lexbor`
both turn it on, and only the bare engine - Kani - is without it),
**`lexbor`** (the vendored Lexbor, only the reference: the generated ABI,
Lexbor's DOM through `lexbor/adapter` and its XPath instance, and the
reference parsers; no build that ships has it, so `nm` finds no `lxb_`
symbol), **`alloc-inject`** (the `rake oom` hook, off in any normal build),
**`css-reference`** (the OLD CSS engine, `lexbor::selectors`, for the
`html_css_diff` fuzz target) and **`lexbor-reference`** (`lexbor::tree_diff`,
`lexbor::selector_diff` and `lexbor::stylesheet_diff`, Lexbor's tree, selector
lists and stylesheets beside the ports', for the `html_diff`, `selector_diff`
and `stylesheet_diff` fuzz targets) and **`xml-css-reference`** (the
CSS-to-XPath lowering `crate::css`, for the `css` fuzz target; no Lexbor). A
plain `cargo build` is the shipping
configuration; `rake rust:test` runs the engine alone, `html-dom` and
`lexbor`. Lexbor's DOM is not an extension build any more
(`MAKIRI_LEXBOR_DOM=1` was retired in stage 9; the tests that held the
arena's edits and XPath to Lexbor's DOM retired with the migration):
`lexbor::tree_port_tests` and the `html_diff` fuzz target hold the tree
builder - and its serializations - to Lexbor's tree node for node, and the
matcher is held to Lexbor's own engine over an arena copy of
Lexbor's parse, node for node by walk position (`lexbor::twin`). The arena's
byte store is append-only between compactions: an edit's start
(`HtmlParsed::edit`, after the indexes - which hold spans - are dropped)
rewrites it once the dead bytes outweigh the live ones
(`html/dom/compact.rs`), so repeated `content=` does not grow a document.
The engine - `xml`, `xpath`, `falloc`, `cbuf`, `cutf8`, `html` (the
tokenizer, the tree builder and the arena), `selectors` (the selector
parser) - is behind no gate at all. So the fuzz crate builds `--no-default-features --features lexbor,css-reference,lexbor-reference,xml-css-reference` and Kani
builds `--no-default-features`. The ~30 features that used to stand here were
migration scaffolding, one per ported C file, and went with the C.

## Subsystems

The HTML DOM below is the arena (`html::dom`, behind the `html_dom` facade,
parsed by `html::tree`); Lexbor's tree is only the reference the parser is
held to (`lexbor::tree_port_tests`, `html_diff`).

**Text-input contract.** A DOCUMENT parse decodes its bytes as a browser
does (`html::encoding` decides, `html::tree::parse_document_bytes` drives):
the HTML Standard's encoding sniffing algorithm
(BOM; the transport layer's encoding - `encoding:`, else the String's own
when it has one; for an ASCII-8BIT String the 1024-byte `<meta>` prescan,
then UTF-8 if the bytes are valid UTF-8, then `chardetng` over 64 KiB
from the first non-ASCII byte, `DETECT_MAX`), "change the
encoding" (a `<meta>` met while the confidence is tentative re-reads the
document), and the Encoding Standard's decoders (`encoding_rs`, into a
`falloc` buffer). `Document#encoding` is what it was read in. A Ruby
encoding the Standard has no label for (UTF-32, CP51932) is transcoded by
Ruby as before (`DocumentSource`, over `html::encoding::transport_for`). Fragment parsing takes text, so
it has no sniffing: there the input String's encoding is honoured
(`ruby_to_utf8_value`, `bridge/string/encoding.rs`): UTF-8 / US-ASCII /
ASCII-8BIT pass through untouched (the UTF-8 common case is one read of the
encoding index from the String's flags - no call, no transcode, no copy), any
other encoding (Shift_JIS, EUC-JP, ISO-8859-1, ...) is
`rb_str_encode`'d to UTF-8 (invalid/undef → U+FFFD) so its content survives
instead of being read as raw UTF-8. After that the bytes are UTF-8 - and a
document's UTF-8 skips the decoder, since the sanitiser below replaces as the
UTF-8 decoder does (`the_sanitiser_is_the_utf8_decoder`). **HTML
parsing then decodes leniently like a browser**: `sanitize`
(`utf8_input.rs`) replaces any remaining invalid UTF-8 with U+FFFD (a NUL is left
for the HTML5 tokenizer to drop/replace), so parse/fragment **never fail** on
bad bytes and the DOM is always valid UTF-8. The validation is a dedicated
validate-only scan (Unicode well-formed table + word-at-a-time ASCII); it is
skipped entirely when the String's cached coderange (read via `ENC_CODERANGE`,
no forced scan) already proves it valid - `parse_html`'s `assume_valid` and
`ruby_str_known_valid_utf8`. The **programmatic APIs are strict**:
`ruby_verified_text` (`bridge/string/view.rs`) raises `Makiri::Error` for **invalid
UTF-8 everywhere** at the XPath/CSS/mutation boundaries (expr, selector,
attribute name/value, `content=`, `create_*`, variable/namespace) -
never truncate/repair. **Embedded NUL (U+0000) is a two-tier contract**: rejected
for names/tags/namespaces/PI target+data/selectors/XPath/variables and all engine
inputs (`text::VerifiedText`, a `&str` with no NUL - a logical contract, since
every consumer, Lexbor's selector parser included, reads a slice and none a C
string), but **accepted for the HTML
data-family** - text/comment node content (`create_text_node`/`create_comment`/
`content=`) and attribute values (`[]=`/`set_attribute_ns`) - so the DOM can hold
U+0000 like browsers. Those data-family sites go through `ruby_verified_data`
(distinct type `RubyData`, UTF-8-validated but NUL-permitting), never
`ruby_verified_text`. A NAME given to a factory or setter goes through
`ruby_verified_name` (XML: `bridge::xml::verified_name`): the same `RubyText`
contract, but its NUL refusal is `ArgumentError`, like every other name
refusal there (the DOM's InvalidCharacterError), so a caller mapping
`ArgumentError` to a DOMException misses none. Removers and readers keep
`ruby_verified_text`. **The strict inputs honour the String's encoding
too** (`utf8_strict`): a String not in UTF-8 / US-ASCII / ASCII-8BIT is
transcoded to UTF-8 first, with NO replacement - a character or byte it
cannot convert is `Makiri::Error` ("could not be converted to UTF-8"), never
repaired - and any other raise in the transcode (`NoMemoryError`, an
interrupt) is passed through as it is. A length cap (`ruby_try_verified_text`)
is on the UTF-8. Both checked views (`RubyText`, `RubyData`) deref to
`&str`, and each OWNS A COPY of the (UTF-8) String's bytes, taken with no
Ruby between the read and the copy (`string::copy_bytes`; up to 64 bytes
inline, so a UTF-8 selector or expression allocates nothing; one in another
encoding allocates its transcode) and checked after. Nothing Ruby
does afterwards - a GC, the caller rewriting or growing the String - reaches
the copy. The HTML source (`HtmlSource`) is a copy too, the
one a document parse needs to read it with the GVL released. The only borrow
of a String's bytes left is `string::with_bytes`, a scoped read for the XML
encoding sniff, whose closure must run no Ruby. (The views used to hold the
String under `rb_str_locktmp`, with a copy when it was already locked, a
frozen fast path and a raise to re-propagate - all to spare a copy of a few
bytes; `spec/mutation_argument_conversion_spec.rb` pins the copy semantics.)
`Makiri::XML`'s data
takes NUL too (`bridge::xml::verified_data`), as the DOM does, and so does every
character XML has no `Char` for: the XML mutators hold what the DOM holds, and
the serializers refuse a tree XML cannot write, deciding from the data as it is
written (`serialize::out`'s byte-class table), never from a flag a setter could
leave stale. XML names stay NUL-strict, like HTML's. Don't drop the
UTF-8 checks or route a name/engine string through the data path; see
`docs/string_types.md`. **No argument is converted**: a string argument is a
`String` (`bridge::ruby::string_arg`), a name - attribute, element, prefix,
selector, expression, encoding - a `String` or a `Symbol` (`name_arg`), and
anything else is a `TypeError`, since a `to_s`/`to_str` is the caller's Ruby.
What converts with `to_s` is `lib/`'s, outside the extension: the parse
entries (`String(source)`, an IO's `read`) and `XML::Builder`. `rake
unsafe:boundaries` counts every call that would run the caller's Ruby
(`USER_CODE_API`); `NodeSet#each`'s `yield` and `XML::Namespace.new` are the
two allowed. The registered methods' parameter types are held to
it by `rake dylint`'s `converted_argument`: a `method!` / `function!` whose
receiver or argument is a type magnus's own `TryConvert` converts (`RString`
by `to_str`, `i64` by `to_int`, `RHash`, `Symbol`, ...) fails; `Value`, `bool`, a
`TypedData` reference and the crate's own `TryConvert` types pass.

**Parsing & source location.** The tree builder stamps an element when it
is CREATED FOR ITS OWN START TAG (`TreeBuilder::created_offset`,
`html/tree/stamp.rs`): a real start-tag token with no base element - never a
`Tok::fake`, an adoption-agency or reconstruction copy, or an element implied
by another tag's token (the `<tr>` a stray `<td>` implies) - so every element
the parser invents stays nil and a void element fostered out of a table has
its line (L1 in `docs/parser_differences.ja.md`).
`lexbor::tree_diff::offsets_name_their_tags` holds it to the one property
`#line` promises: every stamp sits at a start tag of the element's own name.
The offset goes in the node's `source` field (see the gotchas), and a line
table (`source_lines::Lines`, built once per parse) resolves it to a 1-based
line. `Node#line` returns an Integer, or **nil** when unplaceable - never a
wrong line: every element the parser invents, text/comment/attribute nodes,
nodes copied from another document, and fragments (their parse stamps
nothing). The first `#line` is O(1), and a mutation needs no ordering. (The
Lexbor-era design stamped from a token-done hook around Lexbor's tree
builder; an even earlier one paired recorded tokens with elements after the
parse, and parser-made elements stole later tags' lines.)

**HTML parse guard** (`html/tree/guard.rs`). Every parse runs under a
`Guard` the tree builder checks after each token - `Guard::for_document`
from `parse_html`, `Guard::for_fragment` (below the synthetic `<html>`) from
`parse_fragment` - bounding the tree depth (`max_tree_depth:`, `DepthLimit`,
default 400, Nokogiri's). The check is per token, not per insertion, so a
void element at the limit is not refused. There is no limit on a
`<select>`'s options: the selectedness setting algorithm is decided from
each select's state (`html/tree/select.rs`, `SelectStates`), so a select
builds in linear time - keep it so, or the 10,000 limit it replaced has to
come back.

**An attribute's parent is its element**, held in the attribute node's own
`parent` link, which appending the attribute sets and removing it clears
(`html/dom/mod.rs`); `Document::parent` reads it live, so the XPath
parent/ancestor axes, document order and `Attr#parent` all agree and need
no index.

**The indexes** (`html/dom/index.rs`, owned by the parse handle,
`HtmlParsed`) are built lazily and name nodes by id and text by its span in
the byte store, so neither borrows the arena. **Every edit drops them before
anything changes** (`HtmlParsed::edit` -> `invalidate_indexes`), and the
byte store is compacted - moving every span - only right after
(`html/dom/compact.rs`), so no index survives a span it names.
- **element index** (`DomIndex`): a `tag id -> elements` document-order CSR
  over the static tag range, for the XPath `//tag` fast path
  (`xpath/step_index.rs`; `DomIndex::tag_bucket`). A custom tag, a foreign
  element (`has_foreign`) or an HTML element written in upper case
  (`has_case_kept`) sends `//tag` back to the tree walk. Built in one walk
  (each element's tag and id, then a counting sort) by the first `//tag`
  that asks for a bucket (`HtmlDom::name_bucket` ->
  `HtmlParsed::ensure_dom_index`) - never up front, so an XPath that names
  no tag (`/html/body`) pays nothing for it; OOM answers no bucket and the
  step walks. **Every evaluate reads
  the index afresh from the handle**: a reused `XPathContext` that kept the
  one it first saw answered from a dropped index - a use-after-free in the
  Lexbor era (`spec/xpath_context_mutation_spec.rb`).
- **document-order index** (`DocOrder`): each node's position in XPath's
  document order - a node, its attributes, its children; `<template>`
  contents left out, as the XPath walk leaves them - as a dense
  `NodeId -> u32` array sized to the arena. XPath's sorts read it through
  `Dom::KEEPS_ORDER` / `kept_position` / `build_kept_order`
  (`xpath/order.rs`, `sort_by_kept_order`); a node it does not hold
  (detached, a fragment's, appended since) falls back to the parent-chain
  compare. Built by the first sort large enough to want it
  (`INDEX_BUILD_MIN`) and kept across evaluates until an edit drops it;
  OOM falls back for the rest of that evaluate. Counted in
  `HtmlParsed::external_bytes` and reported to the GC once the evaluate
  that built it is over (`account_kept_order`). XML keeps the evaluate's own
  `OrderIndex`. Verify with `spec/xpath_kept_order_spec.rb` and
  `xpath::order::speed::order_speed` (ignored; a measurement).
- **text index** (`TextIndex`): every text node's span in document order and
  each container's `[start, end)` run of them (a dense array by `NodeId`,
  built in one walk), so a `Node#text` is a lookup plus
  `ruby_str_from_slices` - one pre-sized memcpy run, no element node touched
  (**~4x libxml2**). Built only once the walks `Node#text` has served since
  the last edit cover half the document (`HtmlParsed::ensure_text_index` /
  `note_text_walk`): the build costs about one walk of the document, so a
  one-off `li.text` walks its own few nodes instead. Reached via
  `HtmlParsed::text_slices`; `None` (not due, a fragment, a build OOM)
  means the caller walks (`bridge::html::subtree_text`: one walk collecting
  the slices, one copy). Verify with `spec/text_index_spec.rb`, which holds
  it byte-identical to a plain walk across subtrees and mutations.

**Child counting and indexing** (`src/child_index.rs`). `#child_count` /
`#child_at` and their element twins walk `dom_rules::Tree`'s links - the
insertion rules' description of each tree, so the three cannot disagree -
and remember where they were in a per-document `ChildPositionMemo` (a
`PtrMap` per list, emptied past `CHILD_MEMO_MAX`), so an index loop is O(1)
a step. The memo answers only for the `tree_version` it was filled under;
that invariant is what makes a recorded token read back
(`TokenTree::node_of`, a checked lookup, so a stale one is walked afresh
rather than answered) name the right node, so keep every
child-list edit moving the version BEFORE it changes anything
(`record_edit`). Any edit empties the whole memo (the module doc's known
limit). The four Ruby methods are generic over `bridge::wrapper::ChildListHost`
in `glue::node`, and the element-only navigation (`first_element_child`,
`next_element`, ...) is `child_index`'s too, over the same links and element
test.

**XPath engine** (`src/xpath/`). Original implementation: lexer →
recursive-descent parser → AST → evaluator + 26 built-in functions. The only
external hook is `Dom::qualified_name` (in `xpath/dom.rs`). Per-evaluate
budgets (op count, recursion depth, step/predicate/arg counts, node-set & string
caps) live in `xpath/limits.rs` and fail closed with `Status::Limit`.
`enter_recursion` also asks `crate::stack::exhausted()` (installed by
`bridge::stack` at `Init_makiri`; see its module doc): a depth COUNT alone
assumes the caller's frame started with a full-sized native stack, which a
small `Fiber` (`RUBY_FIBER_MACHINE_STACK_SIZE`) or a caller already deep in its
own recursion does not have - there the count refuses too late, and Ruby's
`SystemStackError` handling longjmps past `Drop`. (The CSS lowering's
`Build::enter_selector_nesting` checked the same way while XML's CSS ran on
it; CSS matching, HTML and XML, keeps nesting on a heap stack and needs no
check.) Ruby:
`Node#{xpath,at_xpath}(expr, [namespaces])`, `Makiri::XPathContext`
(`.new`, `#evaluate`, `#register_namespace`/`#register_ns`, `#register_variable`).
`#xpath` returns a NodeSet for node-sets, else String/Float/boolean. Errors map
SYNTAX→`XPath::SyntaxError`, LIMIT→`XPath::LimitExceeded`, else `Makiri::Error`.
**There are no custom functions**: Makiri runs no Ruby while it evaluates.
An unprefixed call the core library lacks is an `XPath::SyntaxError` when the
expression is parsed (`parse_function_call`, over `funcs::is_core_function`),
a prefixed one when it is evaluated (the prefix is bound then), and a
Nokogiri-style handler argument is an `ArgumentError`. The handler bridge
that used to answer them (`rb_protect` per call, a Fiber-suspended walk left
holding the document's guard, a mutation gate on every edit while one ran)
went with it (`notes/ruby_boundary_simplification_plan.ja.md`). The **namespace axis is not
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
it that way; the host's getAttribute (`id()`, `lang()`) has rules of its own. The
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

**CSS** (`css_match/` + `css_cache.rs` + `selectors/`). `Node#{css,at_css,matches?}` (`glue::html_node::css`, `glue::xml_node::css`)
match through `css_match` on both representations - an original,
`#![forbid(unsafe_code)]` Rust port of Lexbor's `lxb_selectors_*` state
machine (its `find`/`found_check`/`not_found` loop over a chain's compounds,
with nested list-pseudos as contexts on a heap task stack; never native
recursion for selector nesting, as `lxb_selectors_run`'s loop never recurses
either), NOT Lexbor's own matcher. Selector PARSING is the parser port
(`crate::selectors`, `docs/rewrite_parser.ja.md` stage 5): Lexbor's CSS
syntax tokenizer, its rule and state stacks and its selector states, line for
line, into an owned index-arena AST read through views of the same shape as
`lexbor::css_parser`'s (`List`, `Lists`, `Selector`, `Simple`, ...), so the
matcher and the XML lowering read either. `lexbor::selector_diff` holds it to
Lexbor's parser view for view (the `selector_diff` fuzz target too); the two
answers that differ on purpose are `docs/parser_differences.ja.md` S2/S3.
The glue reaches it through `css_cache`: an adaptive
compiled-selector cache (`CachePolicy`, the OLD HTML engine's - same
constants, same flush-on-cap/bypass-on-low-hit-rate/retest behavior) holding
each parsed selector it keeps, so a repeated selector string is not re-parsed
every call. `lexbor::css_parser` remains only as the reference: its `parse` and
its views over Lexbor's OWN lists are what `selector_diff` and
`stylesheet_diff` compare against, built only for tests and the reference
features. **`parse_stylesheet` is the port too** (`selectors::parse_stylesheet`
+ `selectors/stylesheet.rs`): Lexbor's rule structure and recovery exactly, but
names and values by one rule for every property - a declaration name
lower-cased unless `--*`, an at-rule name lower-cased, a value the text
between Lexbor's offsets with whitespace runs collapsed - not Lexbor's
per-property reserialization (`docs/parser_differences.ja.md` C1-C3). `lexbor/selectors.rs` (the OLD `lxb_selectors`-callback engine this
replaced for HTML) is the differential-testing reference
(`lexbor::tests::css_match::agrees_with_the_old_lexbor_engine_on_standard_selectors`
and its randomized siblings, and the `html_css_diff` fuzz target) and
nothing else: `#[cfg(test)]` or the `css-reference` feature, so no build
that ships carries it, and without a cache of its own - parse, run, clean
per query. The adaptive cache policy lives only in `css_cache`.
**The matcher is Lexbor's control flow, in safe Rust** (`css_match`'s
`Compiled` and `Query`). `compile` decodes every simple selector once into a
`Step` (Lexbor's entry) and splits every chain - top level and every nested
list - into one compound table; a list-pseudo's `Step` holds the index range
of its alternatives' chains, so matching never looks a list up. A chain is
matched right to left by `Query::step_chain`, Lexbor's `find`/`found_check`/
`not_found` loop: a failed compound backtracks to the nearest descendant or
subsequent-sibling combinator to its right (exhaustive, like Lexbor's). A
`:is()`/`:where()`/`:not()`/`:has()`/`of S` is a nested context - a `Task` on a
heap stack, Lexbor's `lxb_selectors_nested_t` - that the loop returns to with
its verdict, and the rightmost compound is tested before any of that is set
up, so most candidates cost one compound test. An `:is()`/`:where()`/`:not()`
whose every alternative is one compound with nothing nested in it
(`Step::inline`, decided by `compile`) is answered in place, with no task -
`li:not(.r1)` went from 1.37x to 0.84x Lexbor's time. It goes through
`check_compound` rather than calling `check_simple` itself: with a second
caller rustc stopped inlining `check_simple`, and every plain scan got ~10%
slower (`.item`, `main a`) - keep `check_simple` single-caller. **Every table
and stack a query uses is borrowed from a `Scratch`** that the Ruby glue keeps
in `css_cache`'s per-thread engine and hands back
after the query, as Lexbor keeps its entry/nested pools in `lxb_selectors_t`:
a warm `css`/`at_css`/`matches?` allocates nothing but its result. The
vectors cross lifetimes empty, through `recycle` (std's in-place collect,
which reuses the allocation and can never allocate); one grown past
`SCRATCH_KEEP` items is dropped rather than kept. `matches?` was 1.2-2.9x
Lexbor's time before (the allocations - a `Small` spill, the stacks, the
compile work list - were most of a one-candidate call) and is 0.8-1.2x after.
The stacks are cleared, not freed, between candidates; every growth is
`falloc`'s (`MatchFailure::Oom` ->
`Makiri::Error`), `rake oom`'s `css` scenario reaches each site, and there is
no `Box::new`/`Rc`/`vec!` in the file. **Do not bring back a general
continuation machine for plain chains**: the previous design (a frame stack
plus a slab of parked continuations, every compound a frame) was correct and
allocation-free and was still the whole gap to Lexbor - 1.7-3.5x Lexbor's time
on the Ruby-free probe, against 1.0-1.6x after this (`css` bench 4.0k -> 7.9k
i/s). `#id` / `.class` read the element's `attr_id` / `attr_class` shortcut
(`HtmlElement::id_attr`/`class_attr`), as Lexbor does: the DOM's ID and class
attributes, the no-namespace ones - an unprefixed `id` set IN a namespace is
not the ID (`spec/attribute_dom_algorithms_spec.rb`), where the by-qualified-
name lookup used before took it when it was the only `id`.

**One matcher, two DOMs, a browser's answers.** `css_match` reads a DOM only
through `css_match::dom`'s traits (`Host`, `CssNode`, `CssElement`,
`CssAttr`, `CssDoc`), named after the HTML arena's handles, whose impls
(`html/dom/css.rs`) only forward - so the HTML matcher monomorphizes to what
it was; keep `bench:css` flat when touching them. The XML arena's
(`xml/css.rs`) resolves no names (`Host::RESOLVES_NAMES`, bytes compared),
has no `id`/`class` shortcut, and answers a type selector from the document
node out of the XML name indexes (`CssNode::candidates`; `LocalIndex` for a
bare name in any namespace), every candidate re-tested. The rules follow a
browser's `querySelectorAll` (`rake conformance:css_browsers` holds them to
Chromium/Firefox/WebKit): `CssNode::folds_case` - an HTML element in an HTML
document - decides case folding and the 46-name value table, `is_html` (the
namespace) the form states, so an XHTML element in XML has `:checked` but
compares `DIV` as written; a bare type selector matches in any namespace; a
prefix comes from the ns Hash (`css_match::Namespaces`, its `"xmlns"` entry
the default namespace, which a compound with no type selector is kept to as
well, bar an `:is()`/`:where()`/`:not()` argument's subject), resolved at
compile, undeclared = `CSS::SyntaxError` - except inside the forgiving
`:is()`/`:where()`, which drop that alternative alone (`compile`'s `Nesting`);
ancestors outside the context node take part. One `Scratch` per host,
kept by `css_cache` beside the one host-free AST cache.

**Tag and attribute names are resolved to Lexbor's ids once per query**, each
the first time a candidate reaches it - Lexbor's own lazily set `entry->id`
(`css_match::simple::Name`, `HtmlDoc::resolve_attr_name`,
`HtmlElement::attr_by_resolved_name`). Lexbor keys element and attribute
local names by their ASCII-lower-cased form, so a tag-id match IS the
case-folded name comparison, and an attribute local-id match is a pre-filter
the adapter confirms with exactly `attr_by_name`'s comparison - the answer
never changes, only how many name reads it takes. Ids are the document's own
(custom elements and new attribute names are interned per document), so they
are resolved per query in the walked document, and nothing document-specific
is kept in `css_cache`. A tag id is compared without
asking each node for its document, as Lexbor's `entry->id` is: every node a
walking query reaches is in the walked tree, and Makiri never moves a node
between documents (an insert from another document inserts a copy). The
per-node `owner_document` read it replaced cost `css("li")` ~5% (~10% on a
document not in cache). An attribute id is still confirmed by the adapter,
which falls back to bytes for another document's element. `matches?` (one candidate) does not resolve,
even lazily: a lookup costs more than the one comparison it saves
(`ul > li.item` 85 -> 117 ns when tried); it compares the stored local name,
one Lexbor call rather than the two `dom_local_name` makes. An attribute whose
Lexbor `qualified_name` is 0 is named by its lower-cased local name, so a
local-id match confirms without reading the name. Whether a value compares
case-insensitively by default (the 46-name table of
`lxb_selectors_match_attribute_html_case_insensitive`) is settled per selector at
compile time (`Step::value_ci`), as Lexbor's per-id `switch` is - the per-node
table scan it replaced cost `[type=text]` ~40%. A lone top-level chain whose
rightmost compound has a type selector refuses other tags before the machine
starts (`QueryPlan::refuses`, `css_match/plan.rs`).
Measured on the Ruby-free probe against the previous commit: attribute scans
-27 to -31%, type scans -12%, `matches?` -13 to -16%, `.class` and `*` within
+/-6%. Name lookup matches Lexbor's except two documented departures, both the HTML
Standard's case-sensitivity rule where Lexbor folds case on every element: an
attribute NAME is case-sensitive on SVG/MathML, and a TYPE selector is compared
as written to a non-HTML element's localName (lower-cased to an HTML one's), so
`fegaussianblur` misses SVG `feGaussianBlur` (`resolved_names_agree_with_the_old_engine`,
`type_selector_case_follows_the_element_namespace`). The tag-id compare stays
the first test; `written_case_holds` confirms it, and reads no name on an
element with no written name - keep its slow half out of line (inline, it cost
every plain type scan 5-10%).

**`:nth-child(... of S)` is on the heap stack too** (`NthOfTask`): it used to
call a fresh `run` natively per
sibling from `check_simple`, so `of S` nested 300 deep raised
`SystemStackError` in a 128 KiB Fiber and wedged the shared CSS engine for
the process. It also counts by the CSS definition where Lexbor does not (a
comma list, a combinator or a pseudo-class in `S` - `css_match`'s module
doc lists them), so it is checked against a spec oracle
(`nth_child_of_s_agrees_with_a_spec_oracle`), and the Lexbor differential
fuzzer leaves `of S` out. **Every deferred simple selector becomes a task in
one place, `Query::deferred_task`** - a new deferred kind goes there.

**`:has()` nesting is heap-based too, not just `:is`/`:where`/`:not`.** An
earlier version of this port answered `:has()` with ordinary Rust recursion
(`has_forward`), reasoning that `MAX_COMPOUNDS` (64) bounded it safely - true
for ONE `:has()` chain's own compound-by-compound depth, but not for `:has()`
NESTED inside another `:has()`'s argument: `check_simple`'s `Has` arm called
`has_matches` EAGERLY (unlike `:is`/`:where`/`:not`, already deferred to a
heap stack), so each nesting level added a fresh native call
chain with its own 64-deep budget, unbounded by nesting depth. Confirmed as a
real, reachable crash: `:has(` × 300 `div` `)` × 300 against a 302-deep
document, inside a `Fiber` given only `RUBY_FIBER_MACHINE_STACK_SIZE`'s
documented minimum (128 KiB), crashed the WHOLE PROCESS with an uncaught
`SystemStackError` that escaped every `rescue` (exit code 1) - worse than an
ordinary raise, since Ruby's stack-overflow handling `longjmp`s past every
Rust frame between the overflow and the nearest `rb_protect`, skipping `Drop`
(`crate::stack`'s module doc): for `css_cache`'s borrow of its
engine cell, whose `Drop` releases it, a `longjmp`
mid-match would have left it borrowed forever, wedging `Node#css`/
`#at_css`/`#matches?` for the rest of the process - not just crashing the one
call. Lexbor's own C engine never had this problem: `:has()`/`:is()` nesting
is handled by `lxb_selectors_nested_t`, a heap structure, not C recursion
(`lxb_selectors_run`'s loop). Fixed the same way:
`ForwardTask`/`HasCursor` (`css_match/query.rs`) answer `:has()`'s forward
search - candidate iteration over Descendant/Child/NextSibling/
SubsequentSibling, multi-compound stepping, and nesting - entirely on the
SAME heap task stack `:is()` uses, with NO nesting-depth cap
(matching `:is()`'s own "verified safe at 500,000 levels" guarantee, not a
new arbitrary limit). Verified: the differential fuzzer
(`agrees_with_the_old_engine_on_randomly_generated_selectors`, which
generates nested `:has()`/`:is()`/`:where()`/`:not()` with all four
combinators) still agrees with the OLD engine;
`has_nested_inside_has_is_heap_based_not_native_recursion` reproduces the
crash fixture on a 128 KiB thread stack and gets a correct answer, not an
overflow; the original Ruby-level repro (small `Fiber`, depth-300 nesting)
now raises a catchable `Makiri::Error` (`WorkExceeded` - `:has()`'s own
Descendant search exploring a genuinely large candidate space for this
adversarial shape, the SAME pre-existing budget mechanic the old
native-recursive design also relied on, just no longer reachable via a crash
first) and leaves the CSS engine fully usable afterward, never
`SystemStackError`. (Since C20 in `docs/parser_differences.ja.md` the parser rejects a
`:has()` inside a `:has()`, as Selectors 4 does, so the public API no longer
reaches this shape; the matcher still answers it, and the tests that hold it
to Lexbor's engine parse as Lexbor did, `compat::as_lexbor`.)

`select_all`/`select_first`/`matches_any` are **descendant-only** (context node
excluded, like Nokogiri) and in document order; `select_all` is capped at
`NODE_SET_MAX`, and every entry point shares one per-call work `Budget` of
50M steps (XPath's `max_eval_ops`), charged by every compound tested,
`:has()` candidate, sibling counted and fieldset ancestor climbed - exceeding
either raises `Makiri::Error`. It counts a plain chain too, so what keeps
ordinary queries far below it is that no step is repeated needlessly:
`step_chain` does not retry a left part that failed at every ancestor (or
every preceding sibling) from a further one (`Fail`, Blink's
`SelectorChecker` statuses - Lexbor's exhaustive backtracking is exponential
in the chain), and `:nth-child(... of S)` remembers the ranks it counted as
the plain `:nth-*` family remembers positions. A malformed selector raises
`Makiri::CSS::SyntaxError` from the parse step, before matching starts.
`:current()` and the column combinator (`||`) are constructs this
matcher cannot evaluate (deliberately, and because Lexbor's own traversal
can't run the latter either) - raised as "could not be run", never answered as
a silent empty result (`:lexbor-contains()` never gets this far: the parser
rejects it as an unknown function). **Both that and the 64-compound chain cap are decided
by `css_match::compile`, over the WHOLE selector tree, before any node is
matched** - every entry point (`select_all`/`select_first`/`matches_any`)
compiles first, and `matches?` on a non-element compiles too. Found lazily
mid-match they depended on evaluation order: the old `collect_compounds`
returned `None` for an over-cap chain exactly as for an empty one, so `:not(<65
compounds>)` lost its only alternative and matched EVERY element, and `p, x
|| y` answered the `<p>`s because `p` matched first. `compile` walks nested
lists on an explicit work list, not recursion - a recursive first version
turned `:is()` nested 2000 deep into a `SystemStackError` in a 128 KiB Fiber,
which wedged the shared CSS engine for the rest of the process
(`spec/native_stack_guard_spec.rb` pins the Fiber case). **A walking query
remembers the sibling positions it has counted** (`css_match::positions::Positions`,
one memo per `:nth-*` kind): the scan is charged to the budget, and counted
afresh per candidate it made `:nth-child` over a wide list quadratic in that
budget - `tr:nth-child(odd)` over ~4,500 rows raised "exceeded its work
budget". A walk records every counted sibling it passes and stops at the
first known one, so each list is walked once per kind
(`nth_over_a_wide_list_costs_work_linear_in_the_list`); the memo is dropped
and refilled past `POSITIONS_MAX` entries. `matches?` keeps none. **Form
state and links follow the HTML Standard**: `:disabled` /
`:enabled` / `:checked` / `:read-write` are the Standard's definitions
(fieldset inheritance, first `legend` element child, `option`/`optgroup`,
the `input` types `readonly` applies to, `contenteditable`), which Lexbor
only approximates; `:any-link` / `:link` are an HTML `a`/`area` or an SVG
`a` with an `href`, and `:empty` ignores comments and processing
instructions, as browsers have them (M2, M9, M10 in
`docs/parser_differences.ja.md`; `compat::as_lexbor` runs Lexbor's).
**The CSS engine is per thread** (`css_cache`'s `thread_local!`): Ruby runs
one thread at a time under the GVL, so each Ruby thread's engine is reached
by one query at a time, and being the thread's alone it needs to be neither
`Send` nor `Sync` - its kept stacks hold DOM node handles, which are
neither. A re-entrant second borrow on one thread is `ParseError::Busy`
(`try_borrow_mut`), never a second `&mut`. The reference builds' engines
(`css_parser`'s and the OLD `selectors`'s) stay process-global in a
`crate::gvl::GvlCell`, a `Mutex` (`try_lock`, so a re-entrant borrow is
`Busy`, not a deadlock), keyed by a `Gvl` token that `Gvl::exclusive()`
makes outside Ruby; the shipping build has no such cell. (Both used to be
`UnsafeCell`s that only the GVL made sound.)

**Serialization** (`html/dom/serialize.rs`). `Node#{to_html,to_s,outer_html}`
and `#inner_html` are one iterative walk over the arena (`write`), byte for
byte what Lexbor's serializers write but for the documented departures (R1,
R2) - the `html_diff` fuzz target (`tree_diff::check_serialization`) holds
it there: a `<template>` writes its CONTENTS,
not its own children, as the HTML Standard's fragment serializing does;
text is written raw only inside the elements the Standard names
(`raw_text`); the void test reads the tag id (`shape`). `pretty: true` is
the same walk in Lexbor's pretty layout (not the Standard's; it quotes text
nodes). Output goes to a `Sink` chunk by chunk; the shipping one is a
`cbuf::Buf`, pre-reserved only for the document or its root element (sizing
reads the whole arena, so a subtree grows from empty), copied into one UTF-8
Ruby String at the end - faster than appending to a Ruby String, whose
growth is GC-tracked. The ceiling (`ceiling`) is 64 KiB + 32x the bytes the
document owns + nodes x 4 x the longest name: a name is stored once however
many nodes write it, so 50 elements named by 20,000 bytes need the name
term. **The per-chunk append is the hot path** (a chunk is a few bytes,
~20 per element): `Buf::append`'s inline fast path (fits the room => no
ceiling check, since the room never passes the ceiling) and its
`memcpy`-free copy of <= 16 bytes (`copy_small`: two fixed-size copies, safe
Rust over a `Vec` kept initialised to its room) are what put `to_html` ahead
of `nokolexbor`, and the 64-byte first room (`FIRST_ROOM`) is what keeps a
subtree's, which grows from empty, ahead of the libc buffer it replaced;
profile them before touching any of the three. A
`DocumentFragment` serializes as its children. `#inner_html` and
`#inner_html=` special-case an HTML `<template>` to its contents fragment,
as the WHATWG DOM special-cases `innerHTML` alone - so `inner_html`,
`inner_html=`, `#to_html` and `content_fragment` agree, while
`children`/`content=` keep the element's own (empty) children.
`Node#text`/`#content` (`html_node::read::content`) is served from the text
index, with a direct iterative walk for nodes it does not cover
(fragments); for a Document it returns the **root element's** text (the
DOM's null is not what callers want).

**Mutation** (`glue/html_node/mutate.rs` over `html/dom/edit.rs`). Tree edits
(`add_child`/`<<`, `add_previous_sibling`/`before`, `add_next_sibling`/
`after`, `remove`/`unlink`, `replace`) **detach, never destroy** - the arena
owns node memory and live Ruby wrappers may alias a removed node; a move is
detach-then-insert. **Every HTML attribute lookup and write is the DOM's
algorithm** (`HtmlElement::attr_by_name` / `get_attribute`, by qualified
name, lower-cased on an HTML element; `HtmlDocMut::set_attribute` /
`set_attribute_ns`, by namespace + local name), so `svg_a["href"]` does not
find `xlink:href`. The `id`/`class` shortcuts that back CSS `#id` / `.class`
move only through `Document::link_attribute` and a `ShortcutRule` each caller
names (the parser's Lexbor rule, the DOM algorithms' `Take`, a copy's).
There is NO rename (`name=` was removed on both representations): the DOM has
none. Copies - `clone_node`, `import_node`, an insert from another document,
a fragment's import - are `html/dom/copy.rs` (`HtmlDocMut::import`,
`import_xml`). Fragments: `DocumentFragment.parse(html)` (own backing doc)
and `Document#fragment(html)` (bound to a doc) parse in a throwaway document
(`html::fragment`) and import each child; inserting a fragment splices its
**children**. The four structural verbs are one
`bridge::html::insert(this, node, Place)`. The rules - the WHATWG DOM's
"ensure pre-insertion validity" - are written ONCE, in `crate::dom_rules::check`
(with the DOM's other rules both sides must answer alike: `places_itself`,
`text_content`, `removable`), generic over a small `Tree` trait that the
arena (`HtmlTree`: a template's contents reach it through `host`, an
attribute has no tree parent) and the XML arena both implement; each side
keeps only its linking (and XML its namespace resolution), and checks a
fragment as one node before any link changes. The placing is
`HtmlDocMut::place`. **Every edit goes through `bridge::html::edit` and then
`HtmlEdit::with_tree` (or `with_attributes` / `with_data`), which drops the
indexes** (`HtmlParsed::edit`) and lends the document as `&mut HtmlDocMut`
for a closure that runs no Ruby - so no mutator calls it, and none can
forget to on an error path. The factories, copies and fragment imports reach
the document through `bridge::html::new_node` (`HtmlNewNodes`, which keeps
the indexes: a detached node is in none). A `RawNode` is a reference INTO a
document and becomes a handle only through `HtmlDoc::node` / `checked` (or
`HtmlDocMut::node` inside an edit). `edit` and `with_tree` are separate on
purpose: `edit` checks (frozen) so those errors come first, and
every argument is converted BETWEEN the two, because a conversion can
allocate, and so raise, and must not do so once the indexes are dropped. The XML side is the same pair, `begin_edit`
and `Editing::with_arena`. Both take the receiver `&mut` for the edit's
life, so a read of its document through it (`HtmlSelf::read`,
`XmlSelf::read`) still held when the edit begins - one the edit's compaction
or arena growth would invalidate - is a compile-time borrow error; a read the
edit itself needs goes through it (`Editing::read`). Any other read still held
(a copy of the receiver, another node's) is the write
guard's to refuse, at runtime (`bridge::dom`). `inner_html=`/`outer_html=` are all or nothing: `stage_fragment_in`
imports into a DETACHED fragment first, and only then are the old nodes
swapped out (`rake oom`'s `html_inner_html` scenario checks the document is
unchanged after every injected failure).

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
(`encoding` is the one the document was read in, "UTF-8" for one made). `NodeSet#{|,+,&,-}` are identity-dedup,
encounter-order (**not** doc-order), `#{css,xpath,search}` run per node and union
(return `self` when empty so an empty set stays a NodeSet), `#{last,at,remove}`.
`Element.new(name, doc)` / `Text.new(content, doc)` delegate to
`Document#create_*` (they override `.new`, since the allocator is undef'd).

## Performance

**Makiri beats Nokogiri/libxml2 on every `rake bench` row.** Measured
against Nokogiri: parse ~4.6×, css ~10×, at_css ~7000×, `//tag` ~4×,
`//*[@id=…]` ~8×, `[@attr='v']` ~4.3×, attribute axis ~3×, serialize ~6×,
full-text extraction ~3.5×. **traverse** (children walk) used to be the one row
that only met Nokogiri (within measurement error); as of the v0.10.0 bench it
beats it too. (`css`/`at_css` were remeasured after HTML CSS matching moved
from Lexbor's engine to `css_match`, then after the
per-candidate allocation fix and the move to Lexbor's own loop below - see
that note; the `~12×`/`~9400×` figures an earlier revision of this file
quoted were the OLD engine's.)

Treat these as indicative, not precise. Two consecutive runs on the same machine
put full-text extraction at 2.9× and 3.5×, and threaded parse scaling at 2.4×
and 1.7×; benchmark-ips reports ±18-27% on the heavier rows. What the numbers
are good for is catching a *regression in kind* (a row falling to parity or
below), not for defending a second decimal place.

Parsing also scales across threads (~1.7-2.4× on 8 cores) because it releases
the GVL; XPath does not scale, by design (it holds the GVL - see below).

Key decisions that got there, worth not regressing:

- **Parsing releases the GVL; XPath evaluation does NOT** (`bridge::doc`,
  `glue/query.rs`): parse copies the source to a C buffer then runs
  `parse_html` under `rb_thread_call_without_gvl2` - safe because a freshly
  parsed document is not yet shared, so it can't race anything. The `2`
  variant is deliberate: the plain one raises a pending interrupt
  (`Timeout`, `Thread#raise`) as it returns, a longjmp over the Rust frames
  holding the parsed document and the source copy, which leaked both on every
  interrupted parse. `bridge::gvl::without_gvl` delivers an interrupt only
  when the body never started, under `protect`. **XPath holds
  the GVL for the whole evaluation by design** (`xpath::ctx::Session::evaluate` is a plain
  GVL-held call). The engine and DOM are not thread-safe against concurrent
  mutation, and holding the GVL makes that safe *by construction*: the GVL
  serialises all Ruby-thread C code, so an XPath walk never runs in parallel
  with a tree mutation, with another `evaluate` on the same context, or with a
  `register_variable`/`register_namespace`/`node=` on the same context - no
  locking needed (and none is used). An earlier version released the GVL for
  XPath (it scaled queries ~3.3× across threads), but the locking
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

- **HTML CSS matches over the typed adapter, not Lexbor's own matcher**
  (`css_match`, see the subsystem note), and **allocates per
  query, not per candidate**. Right after the switch it was ~6x slower than
  Lexbor's C matcher PER CANDIDATE NODE (a Ruby-free in-process probe, same
  parsed document, same ratio for `css`'s whole-tree scan and `at_css`'s
  first match): the chain was re-collected into a fresh `Vec` and `run` got a
  fresh frame stack for every element visited, and every descendant/`~`
  retry boxed a continuation. Lexbor parses once into an arena and resets an
  object pool per candidate. The fix is the same shape (`Compiled`/`Query`,
  the subsystem note): 0 allocations per extra candidate. The compiled-
  selector cache was tried first and moved nothing (`at_css('#main')`'s parse
  was never the cost).

  Measured against the OLD engine on the same Ruby-free probe (new/old
  time): `ul li.item` 4.5 -> 3.1, `:is(li, p) > a.link` 5.1 -> 2.7,
  `li:has(> span.meta)` 6.3 -> 3.2, `at_css('#main')` 4.9 -> 2.8,
  `at_css('#bot p')` 3.6 -> 2.4; `matches?` on one candidate stays ~1.8 (the
  per-query compile is its whole cost). `bench`: `css` 2.36k -> 4.01k i/s,
  `at_css` 1.35M -> 1.75M i/s. The gap left was frame dispatch: every
  compound of every chain went through the continuation machine. With
  Lexbor's loop instead (the subsystem note) the probe reads new/Lexbor
  1.0-1.6 across `li`, `.item`, `[data-id]`, `ul li.item`, `main a`,
  `li > a.link`, `:is()`, `:not()`, `:has()` and both `at_css` rows (from
  1.7-3.5), and `bench` `css` 4.01k -> 7.95k i/s, `at_css` 1.75M -> 2.02M
  i/s - ~6.6x / ~2.9x nokolexbor, ~10x / ~5000x Nokogiri (nokolexbor's own
  number swings run to run; judge by makiri's absolute i/s). Reusing the
  tables and stacks across calls (`Scratch`, the subsystem note) then took
  `at_css` to 2.70M i/s (~3.8x nokolexbor), `css` 8.38k. Verify with
  `bench`'s `at_css`/`css` rows, the differential fuzzer, `rake oom` and
  `FUZZ_ARGS="--target css" bundle exec rake fuzz:sanitize`.
- **The compiled-selector cache** (`css_cache`):
  `Node#css`/`#at_css`/`#matches?` do not re-parse a repeated selector string
  - the OLD engine's adaptive `CachePolicy` (same constants, same
  flush-on-cap/bypass-on-low-hit-rate/retest behavior), now holding the parser
  port's owned ASTs, where it used to hold lists in a Lexbor arena of its own.
  It matters more than it did: the port parses at 1.5-2.5x Lexbor's time
  (0.25-1.1 us a selector; it allocates its stacks per parse, where Lexbor
  reuses one parser), so a cache hit hides that and only the per-call paths
  pay it - one-off selectors (~5-10% on a uniquely-named `at_css`). XML's CSS
  shares the cache (and has its own `Scratch`, `Kept`). Reusing the parser's work space is
  stage 9's (`docs/rewrite_parser.ja.md`). Verify with
  `spec/css_selector_cache_spec.rb` (flush, bypass/retest, and
  reject-without-disturbing-the-cache) and the Rust-level
  `css_cache::tests`.
- **`Node#text` is served from the text index** (`html/dom/index.rs`,
  see the subsystem note): a per-document, lazily-built, edit-dropped map
  from node → its document-order run of text spans, turning repeated text
  extraction into a lookup + one pre-sized memcpy instead of a walk (~4×
  libxml2). It is built once the walks it would replace add up to half the
  document, so first and one-off calls walk (a first `doc.text` 2x faster,
  a first small `li.text` from a whole-document build to nothing). Do not
  regress to walking on the indexed path; verify
  with `bench`'s "full document text" row and `spec/text_index_spec.rb` (which
  asserts byte-identity with a plain walk across subtrees + mutations).
- **String-value cache is hashed** (`xpath/str_cache.rs`): a token-keyed
  `PtrMap` index over an ordered store, so per-node predicate compares
  are O(1), not the old O(n²) linear scan. The cache belongs to one evaluate
  (`xpath::eval::Evaluation`), as do the op budget and - for XML - the
  document-order index. HTML's document-order index is the document's
  (`DocOrder`, above): no evaluate can edit the document it reads.
- **Node string-values are borrowed when they are one slice** (`xpath/value.rs`
  `node_string_value` -> `Str`, `NodeText::Borrowed`): an attribute, a
  text/CDATA/comment/PI node, and a container whose descendants hold one
  non-empty text (`<li>item 5</li>`) are read in place; only a value joined
  from several texts is built. Leaves and attributes skip the cache (a read is
  cheaper than a lookup); a container's value is still cached, borrowed or
  built, because its walk is charged to the op budget and a node-set
  comparison must not pay it again - not caching borrowed ones measured ~13%
  faster on `//li[. = 'x']` but would re-charge that walk per pair.
  `max_cache_bytes` counts a borrowed entry by its length, and a borrow is held
  to `max_string_bytes` exactly as a build (`Buf::content_limit_for`).
- **`[@name]` / `[@name='lit']` predicates take a direct-attribute fast path**
  (`match_attr_pred`/`attr_pred_matches` in `xpath/attr_pred.rs`): a
  position-independent scan of the element's attributes with the axis's own
  name test, instead of building a throwaway node-set per candidate; anything else falls
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
- **Per-context compiled-AST cache** (`bridge/xpath/context_object.rs`): an `XPathContext` parses
  each expression once and re-runs the cached AST (bounded by `AST_CACHE_MAX`).
  `Node#xpath` uses a throwaway context and does not cache.
- **Every Document reports its arena to the GC** (`account_document`, which
  `DocumentShell::install` calls; `impl Drop for DocData` takes the report back). Neither
  arena, HTML or XML, is an `xmalloc`, so without the report Ruby
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
  both halves. The report is what the document OWNS (`HtmlParsed::external_bytes`,
  the XML arena's `memsize`). Growth through
  mutation is re-reported where an edit begins (`account_growth`, from
  `bridge::html::edit` / `bridge::xml::begin_edit`) once it reaches an eighth
  of the last report (and a floor): for HTML an O(1) measure decides
  (`HtmlParsed::arena_chunks`) before the bytes are measured again. A document's last edit may go unreported
  (under-reporting, the safe direction). Without it, 60 documents grown by
  `inner_html=` ran with zero GCs to ~700 MB RSS; with it, ~200 MB.
- Tree-walk speed was structurally capped by Lexbor's 96-byte node; the arena
  DOM that ships is ~109 B a node all in (byte store and names included,
  against Lexbor's ~180 B) and walks children ~13% faster. Investigated
  nodeset-pool / prefetch follow-ups were **not** shipped because, with no
  remaining slower-than-Nokogiri row, they'd add lifetime / speculative
  complexity for no measurable win. Stage 9's measurements (arena against
  Lexbor's DOM, row by row, and why the selector parser's work space is not
  reused) are in `docs/rewrite_parser.ja.md`; measure with min CPU time over
  repeated runs, since benchmark-ips swings 15-25% on this kind of machine.

Working on perf: capture a `rake bench` baseline, change one thing, re-bench,
and **ship only a measurable win** that keeps `rake spec` + `rake fuzz:sanitize`
green.
