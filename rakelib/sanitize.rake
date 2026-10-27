# frozen_string_literal: true

# AddressSanitizer options for every sanitized run, plus the preload that goes
# with it - which is PLATFORM-SPLIT, because the two systems need opposite
# things and treating them alike breaks one of them.
#
# macOS: NO preload. rustc links its own ASan runtime into the cdylib as an
# @rpath dependency, so dyld loads it together with the extension and nothing
# has to come first. Preloading on top of that breaks three ways:
#
#   * Apple's clang runtime does not export `__asan_version_mismatch_check_v8`,
#     which rustc's instrumentation calls from every image's `asan.module_ctor`.
#     Under `-undefined dynamic_lookup` a missing symbol is not a link error but
#     a NULL pointer, so that constructor jumped to address 0 while dyld ran the
#     image's initialisers - BEFORE `Init_makiri`. That was the load segfault.
#   * Preloading rustc's runtime instead deadlocks inside dyld: its init takes a
#     non-recursive spin lock, maps shadow memory, and the dyld call that does
#     so allocates - re-entering that same init through ASan's own malloc
#     interceptor, which then spins in sched_yield forever.
#   * Preloading clang's alongside rustc's linked one is simply two runtimes.
#
# Linux: the preload is REQUIRED, for the mirror-image reason. rustc links the
# sanitizer runtime into executables but NOT into a cdylib, so the .so carries
# `__asan_*` undefined and expects the host to supply them. With no preload
# `dlopen` fails outright - "undefined symbol: __asan_handle_no_return" - and no
# ASAN_OPTIONS value helps, because the flags govern checks, not symbol
# resolution. GCC's libasan supplies them and exports the same `_v8` ABI that
# rustc's instrumentation asks for, so it is what gets preloaded. Verified on
# linux/amd64: without it the load fails; with it a real heap-buffer-overflow is
# reported with the Rust frame.
#
# `verify_interceptors=0` is what the macOS arrangement costs, and it costs
# nothing real: that check asserts the runtime loaded ahead of libSystem, which
# is false for a library dyld brings in with the extension. The interceptors
# install anyway - a verbosity=1 run reports "libc interceptors initialized"
# with the shadow mapped, redzone=16 and a 256M quarantine. On Linux it is
# inert, the preload having already satisfied the ordering.
# `verify_asan_link_order=0` is the same assertion under another name.
#
# LeakSanitizer stays off - it would flag Ruby's intentional caches, and the
# interpreter is not instrumented. Real heap findings stay fatal.
#
# `use_sigaltstack=0`: Ruby installs its own (malloc'd) alternate signal stack
# on every native thread, replacing the one ASan mmap'd at thread start. When
# such a thread exits, ASan's `UnsetAlternateSignalStack` munmaps whatever stack
# is current - Ruby's heap pointer - gets EINVAL, and dies with "failed to
# deallocate ... unable to unmmap" (seen on Linux as soon as a spec ran XPath
# in a `Thread.new`). Not installing ASan's stack means nothing is unmapped.
ASAN_ENV_OPTIONS = "detect_leaks=0:detect_container_overflow=0:" \
                   "detect_odr_violation=0:verify_interceptors=0:" \
                   "verify_asan_link_order=0:use_sigaltstack=0:" \
                   "abort_on_error=1:halt_on_error=1"

# The ASan runtime to preload. Linux only - on macOS preloading is the thing
# that breaks the run, so this returns nil there by construction rather than by
# a caller remembering to ask.
def asan_runtime_path
  return nil if RbConfig::CONFIG["target_os"] =~ /darwin/

  cc = RbConfig::CONFIG["CC"] || "cc"
  arch = RUBY_PLATFORM[/x86_64|aarch64|arm64/] || "x86_64"
  ["libasan.so", "libclang_rt.asan-#{arch}.so", "libclang_rt.asan.so"].each do |name|
    path = `#{cc} -print-file-name=#{name} 2>/dev/null`.strip
    return path if path != name && !path.empty? && File.exist?(path)
  end
  nil
end

# The preload entry for a sanitized run's environment; empty where none is
# wanted, so every task merges the same call and none repeats the condition.
def asan_preload_env(sanitize)
  return {} unless sanitize.include?("address")
  return {} if RbConfig::CONFIG["target_os"] =~ /darwin/

  runtime = asan_runtime_path or
    abort "sanitize: no ASan runtime found for #{RbConfig::CONFIG['CC']} - a " \
          "sanitized cdylib cannot be dlopen'd on Linux without one."
  puts "sanitize: preloading #{runtime} via LD_PRELOAD"
  { "LD_PRELOAD" => runtime }
end

# The coverage-guided harnesses are a cargo-fuzz crate now (they were C files
# under ext/makiri/fuzz driven by a Makefile). cargo-fuzz supplies libFuzzer and
# the sanitizer itself, so the check is for the tool, not for a working clang.
def cargo_fuzz_available?
  system("cargo", "fuzz", "--version", out: File::NULL, err: File::NULL)
end

# The compiled extension, and whether it carries sanitizer instrumentation, so
# `fuzz:sanitize SKIP_BUILD=1` can refuse to run a plain (non-ASan) build.
def ext_bundle_path
  Dir["lib/makiri/makiri.{bundle,so}"].first
end

# Is the extension Ruby will actually load ASan-instrumented?
#
# The test is `__asan_report_*`, and which symbol is asked for is the whole
# point. An instrumented load or store calls one of those on a failing access,
# so the reference exists only because the compiler instrumented OUR code. The
# interceptor and bookkeeping symbols (`__asan_init`, `__asan_memcpy`,
# `__asan_handle_no_return`) come from merely linking the runtime and appear
# whether or not anything was instrumented - which is why the earlier version of
# this check, grepping the bundle for /asan|ubsan/, was weaker than it looked.
# Measured on this tree: an uninstrumented archive has 0 report references, the
# instrumented one has 72, and the bundle built from it has 8.
#
# It examines the INSTALLED BUNDLE, not a target directory. That bundle is what
# the spec suite loads, so its instrumentation is the claim being made; and it
# cannot be fooled by build output lying around. The version before this one
# globbed `ext/makiri/rust/target/**` - which is where a hand-run `cargo` puts
# things and NOT where rake-compiler builds, since the generated Makefile passes
# `--target-dir target` relative to `tmp/<platform>/makiri/<ruby>/`. It found
# seven stale uninstrumented archives from earlier debugging and refused a
# sanitize run that was correctly instrumented. Failing closed was the right
# direction; looking in the wrong place was not.
#
# One check where there were two, because there is one artefact where there were
# two halves. While C and Rust were linked together, a bundle-level answer could
# not tell you whether the Rust half was plain, so a second look inside the
# crate's own archive earned its keep. It does not any more.
def ext_asan_instrumented?
  bundle = ext_bundle_path or
    abort "sanitize: no built extension under lib/makiri - nothing to check."
  !(`nm -u #{bundle.shellescape} 2>/dev/null` =~ /__asan_report/).nil?
end

# Abort unless the extension really is instrumented. Called by the sanitizer
# tasks after the build, so a green run always means what it looks like it
# means - and in particular on the `SKIP_BUILD=1` path, which reuses whatever is
# on disk and is much the likeliest way to end up fuzzing a plain build.
def assert_sanitized!(task, sanitize)
  return unless sanitize.include?("address")

  unless ext_asan_instrumented?
    abort "#{task}: #{ext_bundle_path} is NOT ASan-instrumented, so this run " \
          "would come out green having checked nothing. Rebuild without " \
          "SKIP_BUILD (extconf passes -Zsanitizer=address when ASan is on)."
  end
  puts "#{task}: ASan covers the extension. UBSan does not run at all - " \
       "rustc's -Zsanitizer has no `undefined`, and there is no C left for " \
       "-fsanitize=undefined to instrument."
end

desc "Build the extension under AddressSanitizer (MAKIRI_SANITIZE, default " \
     "address; needs the nightly toolchain) and run the spec suite under it. " \
     "There is no UBSan mode: rustc's -Zsanitizer has no `undefined`"
task :sanitize do
  sanitize = ENV["MAKIRI_SANITIZE"] || "address"
  sh({ "MAKIRI_SANITIZE" => sanitize }, "#{FileUtils::RUBY} -S rake clean compile")

  # What this run does and does not cover, stated up front: a sanitizer run that
  # quietly skips part of the extension is worse than no run. extconf builds the
  # crate with -Zsanitizer=address when ASan is on (it aborts if nightly is
  # missing rather than silently building it plain), but Rust has no UBSan at
  # all - rustc's -Zsanitizer takes address/cfi/dataflow/hwaddress/kcfi/
  # kernel-address/leak/memory/memtag/safestack/shadow-call-stack/thread/
  # realtime, and `undefined` is not one of them. With the C retired, UBSan has
  # no subject left either; that is permanent, not a gap waiting to be closed.
  assert_sanitized!("sanitize", sanitize)

  env = {
    "ASAN_OPTIONS"  => ASAN_ENV_OPTIONS,
    # Tells the child it is a sanitizer run. Without it spec_helper cannot know,
    # and its `:slow` exclusion - written for Valgrind, where those examples were
    # measured to dominate - never applied here. That is how a suite which takes
    # 18s plain reached an hour under ASan without anything being wrong.
    "MAKIRI_SANITIZE" => sanitize,
  }.merge(asan_preload_env(sanitize))
  sh(env, "#{FileUtils::RUBY} -S rspec")
end

# `rake debug` - the spec suite over a dev-profile extension
# (MAKIRI_CARGO_PROFILE=dev), whose `debug_assert!`s are compiled in. Every
# other run uses the release profile, which drops them: an assertion that
# was itself wrong - `ExceptionClass`'s, which recursed into itself - made a
# debug build overflow its stack on `require`, and nothing noticed. The
# extension is asked whether it has them (`Makiri.__debug_assertions?`), so a
# stale release build cannot pass for one. Like `rake sanitize`, it leaves
# the debug build in place: `rake clean compile` restores a release one.
desc "Build a dev-profile extension (debug_assert! compiled in) and run the spec suite on it"
task :debug do
  sh({ "MAKIRI_CARGO_PROFILE" => "dev" }, "#{FileUtils::RUBY} -S rake clean compile")
  probe = "#{FileUtils::RUBY} -Ilib -rmakiri -e 'exit(Makiri.__debug_assertions? ? 0 : 1)'"
  unless system(probe)
    abort "debug: #{ext_bundle_path} was built without debug assertions, so this run would check none"
  end
  sh({ "MAKIRI_DEBUG_BUILD" => "1" }, "#{FileUtils::RUBY} -S rspec")
end
