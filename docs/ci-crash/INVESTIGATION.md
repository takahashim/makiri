# CI sanitize-job crash investigation

Status: **root-caused and fixed** (ASan stack instrumentation disabled for the
ext build; see "Fix" below). Branch: `fix-debug`.

## Symptom

The push of `f696fd6` ("Add a malloc-leak gate") to `main` turned the two
ASan+UBSan jobs red, in both workflows that run them:

- `CI` → "AddressSanitizer + UBSan (Ruby 3.4)" and "(Ruby 4.0)" (run 27340395750)
- `Security` → "ASan + UBSan security suite" (run 27340395761)

All plain (non-sanitized) jobs stayed green on the same commit. The job log is
not downloadable with the available token (HTTP 403), but the failure reproduces
locally on Linux with `bundle exec rake sanitize` (Ruby 3.4.5, gcc 13.3,
libasan 8).

## Reproduction

`rake sanitize` aborts during `spec/xml_mutation_pbt_spec.rb`
("Makiri::XML mutation invariants (property test)"), which runs
`MutateFuzz.run(seed)` for seeds 0..2999:

```
AddressSanitizer: CHECK failed: asan_thread.cpp:369
  "((ptr[0] == kCurrentStackFrameMagic)) != (0)" (0x0, 0x0)
    #2 __asan::AsanThread::GetStackFrameAccessByAddr
    #3 __asan::GetStackAddressInformation
    ...
    #8  memcpy (ASan interceptor)
    #10 ruby_nonempty_memcpy include/ruby/internal/memory.h:671
    #12 invoke_iseq_block_from_c vm.c:1612        <- Ruby VM, block-arg copy
```

Key facts established while narrowing it down:

- The abort consistently happens around seed ~1884 of the in-process loop, but
  **seed 1884 in a fresh process passes** - the failure depends on accumulated
  process state (stack/heap layout), not on any single input.
- Breaking on `__asan::ReportGenericError` under gdb shows the "error" ASan is
  trying to report when it aborts: a **READ of size 8 at 0x7fffffffc298** - an
  address on the **main C stack**, above the frame pointer of
  `invoke_iseq_block_from_c`, i.e. Ruby's perfectly ordinary copy of block
  arguments from the caller's stack to the VM stack. Not our memory, not heap.
- The ASan CHECK failure itself is secondary: while *rendering* the report,
  ASan classifies the address as stack, looks for the owning instrumented
  frame's magic, finds none, and dies. The report it was building was already
  a false positive.

## Root cause

This Ruby (3.4.5, mise/ruby-build, like CI's setup-ruby builds) is configured
with:

```c
#define RUBY_SETJMP(env) __builtin_setjmp((env))   /* ruby/config.h */
```

so every raise/rescue unwinds via `__builtin_longjmp`. The ASan runtime
intercepts `longjmp`/`siglongjmp` (calling `__asan_handle_no_return`, which
clears stack shadow below the jump target) - but it **cannot intercept
`__builtin_longjmp`**, which compiles to a direct register restore + jump.

Consequence, with an ASan-instrumented ext under an uninstrumented interpreter:

1. An instrumented Makiri C function places red zones (poisoned shadow) around
   its stack locals.
2. `rb_raise` fires while that frame is live - the mutate fuzzer does this
   thousands of times on purpose (its guards' "expected" error category), and
   a Ruby-level exception crossing our frames via `rb_protect` does the same.
3. `__builtin_longjmp` unwinds straight past the frame; its epilogue (which
   would unpoison the shadow) never runs. The poison stays.
4. Eventually an ASan *interceptor* inside the uninstrumented interpreter
   (here: `memcpy` copying block args) touches a C-stack address whose stale
   shadow still says "redzone" → spurious report → CHECK abort while
   describing it.

This is layout-sensitive (stack depths must line up), which explains both the
"works at seed N in isolation" behaviour and why an unrelated push flipped CI:
none of the four commits in the failing push (`8ef2250` GC-guard removals,
`4334e63` CSS callback, `3480983` leak plugs, `f696fd6` rake task) touches code
the XML mutate fuzzer can even reach - verified by reading the fuzzer (XML
surface only: no XPath, no CSS, no HTML mutation, no Fiber/Thread) against each
diff. The bug is latent in the *tooling combination*, not a regression in
Makiri; the push merely shifted code/stack layout enough to make the dice land.

## Fix

`ext/makiri/extconf.rb`: when `MAKIRI_SANITIZE` includes `address`, build our
sources with stack instrumentation off:

- gcc: `--param asan-stack=0`
- clang/macOS: `-mllvm -asan-stack=0`

No stack red zones → nothing for `__builtin_longjmp` to leave behind, on any
unwind path (our raises, handler raises through `rb_protect`, future Fiber
use). What we keep is everything the sanitize gate is actually for:

- heap red zones (every malloc in our code *and* in uninstrumented Lexbor),
- UBSan,
- the manual arena poisoning in `mkr_xml_node.c` (heap memory - unaffected),
- LSan-free leak checking via `rake leaks` (separate gate).

What we lose: ASan detection of stack-buffer overflow *in our own ext code*.
`-fstack-protector-strong` still covers the smashing class, and the engine's
stack discipline (no VLAs, bounded recursion via explicit budgets) limits the
exposure. Revisit if CRuby ever switches off `__builtin_setjmp` (then plain
`longjmp` interception would make full stack instrumentation safe), or wire
`__asan_handle_no_return()` into a raise wrapper - rejected for now because it
cannot cover Ruby-level raises crossing our frames.

## Verification

- Before: `rake sanitize` aborts reproducibly (twice, same signature).
- After: full `rake sanitize` green (spec suite incl. the XML mutation PBT).
