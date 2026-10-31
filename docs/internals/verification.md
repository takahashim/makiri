# Build and test tasks, annotated

The rake tasks are in `rakelib/*.rake`, grouped by what they check (Rake loads
them after `Rakefile`, which keeps `GEMSPEC`, `ROOT` and the extension task).
The list below gives each command; the sections after it explain the tasks
that need more than a line.

```bash
git submodule update --init        # fresh clone only; the reference Lexbor
bundle install
bundle exec rake compile           # the crate alone - the extension links no Lexbor
bundle exec rake lexbor            # the reference Lexbor (cmake)
bundle exec rake spec
bundle exec rake api:record        # re-record spec/api_surface.txt with an API change
bundle exec rake rust:test         # the Rust crates' tests
bundle exec rake nupur:spec        # nupur alone, through its own test extension
bundle exec rake lint              # cargo clippy -D warnings everywhere + fmt
bundle exec rake unsafe:boundaries # each file's unsafe count, the forbid files, static mut
bundle exec rake unsafe:fix        # rewrite the recorded unsafe counts and forbid list
bundle exec rake dylint            # the crate's own lints, then their self-test
bundle exec rake clean             # wipe the build dir
bundle exec rake clean:lexbor      # wipe vendor/lexbor/{build,dist}
bundle exec ruby -Ilib -r makiri -e 'p Makiri::VERSION'   # smoke load

bundle exec rake deny              # cargo-deny over the extension's dependencies
bundle exec rake symbols           # THE export gate
bundle exec rake no_unwind         # the proof that nothing that must not unwind can
bundle exec rake diff              # answers vs the recorded C-build baseline
bundle exec rake sanitize          # rebuild w/ -Zsanitizer=address (nightly), run suite
bundle exec rake debug             # rebuild with debug_assert!, run the suite
bundle exec rake fuzz              # robustness fuzzer (spec/fuzz/); FUZZ_ARGS to tune
bundle exec rake fuzz:xml          # the fuzzer's XML parser target; FUZZ_ARGS to tune
bundle exec rake fuzz:mutate       # the fuzzer's XML mutation target; FUZZ_ARGS to tune
bundle exec rake invariants        # randomized property checks (spec/invariants/)
bundle exec rake fuzz:sanitize     # fuzz under ASan - the engine's memory-safety net
bundle exec rake fuzz:libfuzzer    # cargo-fuzz harnesses
bundle exec rake leaks             # macOS malloc-leak gate
bundle exec rake oom               # OOM-injection sweep, then the raise sweep
bundle exec rake miri              # the Ruby-free unsafe code's tests under Miri
bundle exec rake verus             # Verus proofs (ext/makiri/rust/verus/)
bundle exec rake rust:exhaustive   # the tests that run every input, in release
bundle exec rake coverage          # llvm-cov report of our sources over the suite (macOS);
                                   # leaves the instrumented build: `rake clean compile`
bundle exec rake bench             # perf vs Nokogiri (bench-only gems; runs outside bundle)
```

Requires CRuby >= 3.3, a **Rust toolchain** (`cargo`) and libclang (rb-sys
runs bindgen); `cmake` only for the reference Lexbor (`rake lexbor`). The gem
carries none of Lexbor's sources, only its LICENSE and NOTICE, which the ports
owe it. A source install needs cargo too, and `extconf.rb` requires `rb_sys`
at install time; that is why `rb_sys` is a RUNTIME gemspec dependency. Binary
gems are published per platform, so most users never compile.

## Building and cleaning

- `rake lexbor` builds the reference Lexbor through `script/build_lexbor.rb`
  (cmake). `lint`, `rust:test`, `conformance:parser` and the diff fuzzers need
  it and depend on it.
- `rake clean` wipes the build dir; the next compile regenerates the Makefile.
- `rake clean:lexbor` wipes `vendor/lexbor/{build,dist}`, so the next
  `rake lexbor` is a full Lexbor rebuild.

## Tests

- `rake rust:test` runs the tests of the engine (bare and as shipped), falloc,
  Nupur and the Lexbor reference, and the fuzz-crate check.
- `rake nupur:spec` tests Nupur alone, through its test extension
  (`nupur/test/ext`, a cdylib over the layer and nothing of Makiri's) and the
  specs beside it: trampolines, panics, exits, slots under GC, checked String
  reads, arguments.

## Lints and dependency checks

`rake lint` runs `cargo clippy -D warnings` over every crate and configuration
(the workspace, the engine bare and as shipped, the reference, Nupur's test
extension, the fuzz crate), plus fmt and Nupur's rustdoc with `-D warnings`.

### `rake dylint`

`rake dylint` runs the crate's own lints (`lints/makiri_lints`) on the
compiled code, by resolved path. A sanctioned site is an allow with its
reason. The lints:

- `ruby_value_on_heap`: a Ruby value in a heap container, a static or a
  wrapped object's data.
- `ruby_layer_in_engine`, `unguarded_no_unwind_site`,
  `ruby_in_gvl_released_body`, `infallible_allocation`.
- `raising_call_outside_protect`: any rb-sys fn in neither `NON_RAISING` nor
  `REVIEWED`, outside `raw::call`. The tables are held to the C API raise
  survey, vendored, and to Ruby's headers as rb-sys's bindings carry them
  (`script/check_non_raising.rb`).
- `unraising_call_in_raw`, `raw_ruby_outside_bridge`, `runs_callers_ruby`,
  `raw_node_crossing`, `ruby_surface_in_bridge`.
- `discarded_ruby_error`: an `Err` of `bridge::rb::Error` thrown away by
  `.ok()` / `.unwrap_or*` / `.is_ok()` / `let _ =` / `let _name =` / an
  Ok-only or `Err(_)` pattern / a `map_err(|_| ..)` / `drop`. It is FORBIDDEN
  in the extension, so no allow: handle the error with `Error::rescue`, which
  hands every throw/break/kill back. Nupur denies it; its drops are
  `describe::for_message` and the `yield_dropping_error_for_test` hook.
- `ffi_without_gvl_proof` (in Nupur): a safe fn calling `rb_sys::` whose
  signature holds no `&Ruby`, Ruby value or handle made from one. `extern "C"`
  entries and unsafe fns are exempt.
- `c_integer_outside_int`: an integer `as` or a named `c_long` at the Ruby
  boundary outside `nupur::int`.
- `gvl_token_outside_entry`: the GVL's token, `Ruby`, made anywhere but where
  Ruby calls in - `get_unchecked` (crate-private) in `rb::method`, `finish` in
  a `method!` / `function!` expansion, `run_init` in an
  `extern "C" fn Init_*`.
- `forgotten_handle`: a `Typed` / `Kept` given to `mem::forget` or
  `ManuallyDrop`. A handle is the only way to a payload.

The task then runs their self-test (`src/lint_probe.rs`,
`nupur/src/lint_probe.rs`): each `//~` line must be reported by exactly the
lints it names. It needs the nightly that `rust-toolchain.toml` pins, plus
cargo-dylint and dylint-link.

### `rake deny`

`rake deny` runs cargo-deny over the extension's dependencies
(`ext/makiri/rust/deny.toml`): RustSec advisories and yanked releases,
licenses, crates.io only. CI runs it in `security.yml`, nightly too. An
advisory that does not apply is ignored in `deny.toml` with its reason.

## Gates: `rake symbols` and `rake no_unwind`

`rake symbols` is THE export gate. `rake no_unwind` is a release build
proving that no `extern "C"` fn or GC-run `Drop` can unwind
(`nupur/src/no_unwind.rs` for Nupur's sites, `engine/src/no_unwind.rs` for
the engine's). `ruby_boundary.md` explains both.

## `rake diff` and the C baseline

**`rake diff` is not an ordinary test.** `spec/differential/baseline/` holds
what the **C implementation answered**, recorded while it still existed, and
the probes compare this build against it. It cannot be re-recorded, because
there is no second implementation left, so a mismatch is a finding to
investigate, never something to refresh. If a difference is genuinely
intended, edit the baseline in the same commit as the behaviour change, so it
reviews as a behaviour change rather than as a regenerated blob. It is the
check that caught an ASCII-8BIT string, a quoted error message and a lost NUL
terminator that the 1001-example suite passed over.

## Sanitizer and debug builds

How `rake sanitize` starts a run, and why that differs between Linux and
macOS, is in `build_and_sanitizers.md`.

`rake debug` rebuilds as a dev profile, with `debug_assert!` compiled in
(`Makiri.__debug_assertions?` reports it), and runs the suite bar
`:timing`/`:slow`. Every other run is release. The task leaves the debug
build in place; `rake clean compile` goes back to release.

## Fuzzing and property checks

- `rake fuzz:sanitize` rebuilds under ASan and runs the robustness fuzzer over
  five surfaces: XPath over the default fixtures, `css`, `xml`, `mutate` and
  `xmlcss` (CSS over an XML document). `FUZZ_TIME` sets the seconds per
  surface (default 90; CI's `security.yml` passes 30). Each surface runs
  isolated (a fork per query) by default; `FAST=1` (or `ISOLATED=0`) runs in
  process. `SKIP_BUILD=1` reuses the current build and refuses one that is not
  a sanitizer build, and `FUZZ_ARGS` runs one custom invocation instead of the
  five surfaces.
- `rake fuzz:xml` and `rake fuzz:mutate` run the plain fuzzer's `xml` and
  `mutate` targets alone.
- `rake fuzz:libfuzzer` runs the cargo-fuzz harnesses (needs cargo-fuzz and
  nightly). `TARGETS=css,html_xpath` narrows the set; `FUZZ_TIME` sets the
  time per target, or `FUZZ_BUDGET` the total. ASan is on for Linux only: an
  ASan harness deadlocks in dyld on macOS, so macOS runs `-s none` (see
  `rakelib/fuzz.rake`).
- `rake invariants` runs randomized property checks (`spec/invariants/`):
  namespaces, copies (clone/import/adopt/fragment), tree shape, index
  staleness, serialization, the text-input contract. `INVARIANT_COUNT` tunes
  the sweep; `invariants:sanitize` runs them under ASan.

### Fuzzer files

The fuzzer's `spec/fuzz/*.rb` are deliberately not `*_spec.rb`, so `rake spec`
ignores them; findings land in `spec/fuzz/regressions/` (gitignored).

## Leak checks

ASan runs with `detect_leaks=0`, so two other checks find leaks:

- `rake leaks`, the macOS malloc-leak gate, flags per-call leak stacks through
  the ext, including rescued raises.
- On Linux, Valgrind (`spec:valgrind`) flags any "definitely lost" through the
  ext. CI runs it nightly.

## `rake oom`: OOM and raise injection

The OOM-injection sweep rebuilds with `MAKIRI_ALLOC_INJECT=1` and fails each
core alloc site in turn. Every OOM branch must fail closed (a clean raise or a
baseline-identical result); the sweep then reuses the same document/context
after `GC.compact`.

The raise sweep follows (`script/check_raise_injection.rb`). Each allocating C
call of Nupur's `raw/` raises `NoMemoryError` in turn, and no document may be
left borrowed (`Makiri.__borrowed?`) or unusable. This is the runtime check of
the guards-across-raises rule.

Prefix the task with `MAKIRI_SANITIZE=address` to combine it with ASan. The
task leaves the injection build in place; `rake clean compile` goes back to a
normal build.

## Proofs: Miri, Verus and the exhaustive tests

### `rake miri`

`rake miri` runs the tests of the Ruby-free unsafe code - the falloc crate,
with Nupur's `utf8_copy` - under Miri with strict provenance (nightly + miri;
CI: `verify.yml`). `utf8_copy` is Nupur's and std-only; falloc's test build
includes it through `#[path]`.

### `rake verus`

`rake verus` runs the Verus proofs (`ext/makiri/rust/verus/`), which are
unbounded. They cover falloc's `grow_capacity`; `ptr_table`'s probing and load
factor; `ceiling_buf`'s `Buf` (content, ceiling, fail-closed) with
`copy_small`; `limits::admit`, the one rule every work budget charges
through; the strict UTF-8 decoder `decode1` and the XML Char scan built on
it, against vstd's own statement of UTF-8 (`vstd::utf8`); the XML character
classes' inclusions (NameStartChar in NameChar in Char); the XPath
Number gate `xpath::number::extent`; and the word-at-a-time scans, the
tokenizer's `seek4` and the line table's `lines_build`, whose bit tricks no
test can cover word by word. Each is copied and checked against the
source (`verus:sync`).

`VERUS=path/to/verus` names the binary; `rakelib/verus.rake`'s
`VERUS_VERSION` is the version CI uses. CI runs it in `verify.yml`: heavy
tools stay in CI, like Miri.

Vacuity is checked by what must FAIL: an `assert(false)` at each
`// [canary]` mark, one variant each, and the callers in `verus/canaries/`.
What each proof trusts (`external_body`, `assume_specification`, `assume`)
is pinned (`VERUS_TRUSTED`).

### `rake rust:exhaustive`

`rake rust:exhaustive` runs the Rust tests that check every input of their
domain - too many for a debug build, a few seconds in release (CI:
`verify.yml`). Today that is one: `decode1` against std's `from_utf8` on
every buffer of up to four bytes, the tie between vstd's statement of UTF-8,
which Verus proves `decode1` against, and the validator the crate calls,
which Verus has no contract for. Each such test is `#[ignore]`d, so
`rust:test` leaves it out, and lives in a module named `exhaustive`.

Kani proved these properties before Verus did, and CBMC before Kani, over
the C. Each property went to the tool that holds it at every length; the
one left is checked by running every input.
