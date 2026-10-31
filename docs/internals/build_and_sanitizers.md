# Build gotchas and AddressSanitizer

## AddressSanitizer: the preload is platform-split

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
  load segfault - the same `dynamic_lookup` hazard recorded under "Adding a source file" below,
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
`libasan`). `rakelib/sanitize.rake`'s `asan_preload_env` is the single place that decides,
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

## Build / runtime gotchas (read before debugging weirdness)

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
  `undefined` mode any more - see `ruby_boundary.md`.
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
- **The XML arena (`engine/src/xml/arena.rs`) needs no ASan poisoning, and has none.**
  The C build's bump arena did (poisoned 64 KiB chunks, the Lexbor-mraw blind
  spot); the Rust one is an index arena - a `Vec<Node>` and a `Vec<u8>` under
  `#![forbid(unsafe_code)]`, addressed by `NodeId` and spans - so a write past
  one node or byte span is a bounds-checked index, not an overrun into the
  next cut. Everything else we write allocates through `falloc` onto the system
  allocator, or - for the glue's Ruby-side storage - through Ruby's xmalloc;
  ASan red-zones both per allocation.
