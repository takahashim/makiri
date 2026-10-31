# Build and test tasks, annotated

The rake tasks are in `rakelib/*.rake`, grouped by what they check (Rake loads
them after `Rakefile`, which keeps `GEMSPEC`, `ROOT` and the extension task).

```bash
git submodule update --init        # fresh clone only; the reference Lexbor
bundle install
bundle exec rake compile           # the crate alone - the extension links no Lexbor
bundle exec rake lexbor            # the reference Lexbor (script/build_lexbor.rb,
                                   # cmake); lint, rust:test,
                                   # conformance:parser and the diff fuzzers need it
                                   # and depend on it
bundle exec rake spec
bundle exec rake rust:test         # the engine (bare and as shipped), falloc, nupur and the
                                   # Lexbor reference's tests, and the fuzz-crate check
bundle exec rake nupur:spec        # nupur alone: its test extension (nupur/test/ext, a
                                   # cdylib over the layer and nothing of Makiri's) and
                                   # the specs beside it - trampolines, panics, exits,
                                   # slots under GC, checked String reads, arguments
bundle exec rake lint              # cargo clippy -D warnings over every crate and configuration
                                   # (the workspace, the bare engine, the reference) + fmt
bundle exec rake unsafe:boundaries # each file's unsafe count, the forbid files, static mut
bundle exec rake dylint            # the crate's own lints (lints/makiri_lints), on the
                                   # compiled code by resolved path: ruby_value_on_heap
                                   # (a Ruby value in a heap container, a static or a
                                   # wrapped object's data), ruby_layer_in_engine,
                                   # unguarded_no_unwind_site, ruby_in_gvl_released_body,
                                   # infallible_allocation, raising_call_outside_protect
                                   # (any rb-sys fn not in NON_RAISING, outside
                                   # raw::call; the tables are held to the C API
                                   # raise survey, vendored, and to Ruby's headers
                                   # as rb-sys's bindings carry them -
                                   # script/check_non_raising.rb), unraising_call_in_raw,
                                   # raw_ruby_outside_bridge, runs_callers_ruby,
                                   # raw_node_crossing, ruby_surface_in_bridge,
                                   # discarded_ruby_error (an Err of bridge::rb::Error
                                   # thrown away by .ok() / .unwrap_or* / .is_ok() /
                                   # let _ = / let _name = / an Ok-only or Err(_) pattern / a
                                   # map_err(|_| ..) / drop; FORBIDDEN in the
                                   # extension, so no allow: handle with
                                   # Error::rescue, which hands every throw/break/
                                   # kill back; nupur denies it, its one drop being
                                   # describe::for_message), ffi_without_gvl_proof (in nupur: a
                                   # safe fn calling rb_sys:: whose signature holds
                                   # no &Ruby, Ruby value or handle made from one;
                                   # extern "C" entries and unsafe fns exempt),
                                   # c_integer_outside_int (an integer `as` or a
                                   # named c_long at the Ruby boundary outside
                                   # nupur::int), gvl_token_outside_entry (the
                                   # GVL's token, `Ruby`, made anywhere but where
                                   # Ruby calls in: `get_unchecked` - crate-private
                                   # - in `rb::method`, `finish` in a `method!` /
                                   # `function!` expansion, `run_init` in an
                                   # `extern "C" fn Init_*`), forgotten_handle (a
                                   # `Typed` / `Kept` given to `mem::forget` or
                                   # `ManuallyDrop`; a handle is the only way to a
                                   # payload)
                                   # (a sanctioned site is an allow with its reason);
                                   # then their self-test (src/lint_probe.rs,
                                   # nupur/src/lint_probe.rs: each `//~` line
                                   # reported by exactly the lints it names).
                                   # Needs the nightly
                                   # rust-toolchain.toml pins + cargo-dylint/dylint-link
bundle exec rake clean             # wipe the build dir (regenerates the Makefile next compile)
bundle exec rake clean:lexbor      # wipe vendor/lexbor/{build,dist} (full Lexbor rebuild)
bundle exec ruby -Ilib -r makiri -e 'p Makiri::VERSION'   # smoke load

bundle exec rake deny              # cargo-deny over the extension's dependencies
                                   # (ext/makiri/rust/deny.toml): RustSec advisories and
                                   # yanked releases, licenses, crates.io only. CI:
                                   # security.yml, nightly too. An advisory that does not
                                   # apply is ignored in deny.toml with its reason
bundle exec rake symbols           # THE export gate - see `ruby_boundary.md`
bundle exec rake no_unwind         # release build proving no extern "C" fn / GC-run Drop
                                   # can unwind (engine/src/no_unwind.rs) - see `ruby_boundary.md`
bundle exec rake diff              # answers vs the recorded C-build baseline (see below)
bundle exec rake sanitize          # rebuild w/ -Zsanitizer=address (nightly), run suite
bundle exec rake debug             # rebuild as a dev profile (debug_assert! compiled
                                   # in; asked via Makiri.__debug_assertions?) and run
                                   # the suite bar :timing/:slow; every other run is
                                   # release. Leaves the debug build: `rake clean compile`
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
                                   # macOS runs `-s none` (see rakelib/fuzz.rake)
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
                                   # each allocating C call of nupur's raw/ raises
                                   # NoMemoryError in turn, and no document may be left
                                   # borrowed (`Makiri.__borrowed?`) or unusable - the
                                   # runtime check of the guards-across-raises rule.
                                   # Prefix with MAKIRI_SANITIZE=address to combine ASan.
bundle exec rake miri              # the Ruby-free unsafe code's tests - the falloc crate,
                                   # with nupur's utf8_copy - under Miri, strict provenance
                                   # (nightly + miri; CI: verify.yml). utf8_copy is nupur's,
                                   # std-only, and included in falloc's test build (`#[path]`)
bundle exec rake verus             # Verus proofs (ext/makiri/rust/verus/): falloc's
                                   # grow_capacity, ptr_table's probing and
                                   # load factor, ceiling_buf's Buf (content, ceiling,
                                   # fail-closed) with copy_small, and limits::admit,
                                   # the one rule every work budget charges through, copied and checked against the source
                                   # (`verus:sync`) - unbounded.
                                   # VERUS=path/to/verus (rakelib/verus.rake's VERUS_VERSION); CI:
                                   # verify.yml. Heavy tools stay in CI, like Miri and Kani.
                                   # Vacuity is checked by what must FAIL: an
                                   # assert(false) at each `// [canary]` mark, one
                                   # variant each, and verus/canaries/'s callers; what
                                   # each proof trusts (external_body,
                                   # assume_specification, assume) is pinned
                                   # (VERUS_TRUSTED)
bundle exec rake kani              # Kani proofs over the Ruby-free core (needs cargo-kani).
                                   # The successor to the C-era CBMC harnesses: UTF-8
                                   # validate/decode, XML characters, XPath numbers
                                   # (the allocator and ceiling_buf are Verus's now).
                                   # KANI_GROUPS=utf8,xml,xpath picks groups
                                   # (rakelib/rust.rake's KANI_GROUPS, with the code each
                                   # depends on); CI runs only the groups whose
                                   # code changed (`rake kani:groups`), and all of
                                   # them weekly. KANI_JOBS=N runs N in parallel.
                                   # Every harness states with kani::cover! that its
                                   # claim is reached, and the task fails on a harness
                                   # with no cover or an unsatisfied one - a proof an
                                   # over-strong assume made vacuous still says "0 failed"
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


## Fuzzer files

The fuzzer's `spec/fuzz/*.rb` are deliberately not `*_spec.rb`, so `rake spec`
ignores them; findings land in `spec/fuzz/regressions/` (gitignored).

