# frozen_string_literal: true

require "bundler/gem_tasks"
require "rspec/core/rake_task"
require "rake/extensiontask"
require "shellwords"
require "tmpdir"

GEMSPEC = Gem::Specification.load("makiri.gemspec")

# Replace bundler/gem_tasks' `release` (which builds a source-only gem and
# `gem push`es it from the dev machine) with a tag push: it hands the build,
# GitHub Release, and the approval-gated RubyGems publish off to CI
# (.github/workflows/release.yml). Nothing is pushed to RubyGems locally.
Rake::Task["release"].clear
desc "Tag v#{GEMSPEC.version} and push it; CI builds, releases, and publishes"
task release: %w[release:guard_clean release:source_control_push] do
  puts <<~MSG

    Pushed tag v#{GEMSPEC.version}. GitHub Actions (release.yml) will now:
      1. build the source gem + precompiled native gems,
      2. create the GitHub Release and attach them, then
      3. publish to RubyGems via OIDC - after the `rubygems` environment approval.
    Approve the pending deployment in the Actions run to publish; nothing is
    pushed to RubyGems from this machine.
  MSG
end

# The extension is one Rust crate, and this is still rake-compiler's ordinary
# task: it runs ext/makiri/rust/extconf.rb, which calls `create_rust_makefile`.
# The cargo integration lives there, which is where the build's other decisions
# (the vendored Lexbor build, the link arguments, the export trim) already are.
#
# NOT `RbSys::ExtensionTask`, and that is a decision rather than an oversight.
# It is rake-compiler's task plus cross-compilation plumbing - which this
# project does not use: `script/build_native_gem.rb` assembles the precompiled
# gems from binaries CI has already built, with plain `Gem::Package.build`. What
# it would cost is structural: it infers the crate by running `cargo metadata`
# in the REPO ROOT (rb_sys/cargo/metadata.rb passes no working directory), so it
# needs a root workspace manifest; it then looks the crate up by PACKAGE name,
# so `makiri_rs` would have to be renamed; and a root workspace would swallow
# the cargo-fuzz crate, which declares no `[workspace]` of its own, so that
# would need excluding too. Three changes to the crate's shape to gain a
# capability we do not use.
#
# `source_pattern` is what makes `rake compile` notice an edited .rs. The
# Makefile that extconf writes re-runs cargo on every build anyway (cargo does
# its own dependency tracking), but rake-compiler decides whether to invoke make
# at all, and its default pattern is for C.
Rake::ExtensionTask.new("makiri", GEMSPEC) do |ext|
  ext.lib_dir        = "lib/makiri"
  ext.ext_dir        = "ext/makiri/rust"
  ext.source_pattern = "**/*.{rs,toml}"
end

RSpec::Core::RakeTask.new(:spec)

task default: %i[compile spec]

# `rake spec:valgrind` - run the spec suite under Valgrind memcheck via
# ruby_memcheck (Linux CI; see .github/workflows/valgrind.yml). The gem ships
# Ruby's own Valgrind suppression files (matched by Ruby version) and filters
# the report down to errors whose stack touches our extension, so we no longer
# have to fetch ruby.supp from ruby/ruby (that path was removed upstream).
#
# We keep this job's historical contract: catch *use of uninitialised values*
# and *invalid reads/writes* (incl. intra-arena overflows) - NOT leaks (leak
# detection stays with `rake leaks`). So we override ruby_memcheck's defaults,
# which disable undef-value errors and turn on full leak-check.
#
# `filter_all_errors: true` is essential: by default ruby_memcheck only applies
# its "stack must touch the makiri binary" filter to *leak*-kind errors
# (`ValgrindError#should_filter? = filter_all_errors? || kind_leak?`), so every
# uninitialised-value report is surfaced regardless of where it comes from. Ruby's
# conservative GC (machine-context scan, RVALUE flag aging, free-at-exit teardown)
# legitimately reads uninitialised words, and the bundled ruby.supp does not cover
# the free-at-exit / subprocess stacks the `:isolated` specs spin up under
# `--trace-children=yes` - which buried the run in ~3500 pure-Ruby false positives.
# Filtering all error kinds by the same binary-touch rule keeps the gate scoped to
# *our* code: a real uninit/invalid access in mkr_*/Lexbor still has a makiri frame
# and is still reported.
#
# BUT the binary-touch filter is too coarse for one residual class: when a GC
# cycle fires *inside* one of our allocations (or marks through our mark
# callback), CRuby's conservative collector legitimately reads uninitialised
# words (machine-stack scan reading stale frames, incremental mark/sweep reading
# not-yet-written RVALUE flags) while a makiri frame sits on the stack - so ~190
# of these pure-Ruby-GC false positives pass the filter. The gem's bundled
# ruby.supp only covers `each_location*` under Addr8, not the Cond/Value8 reads
# we hit. `suppressions/ruby.supp` (auto-loaded by ruby_memcheck: it globs
# `<dir>/<ruby-version>.supp`, and the bare `ruby.supp` matches every version)
# suppresses exactly those GC-driver-anchored uninit reads, plus the VM
# method-cache id_table the interpreter never frees before exit. A real uninit
# read in our code does not descend from a GC driver, so it still fails.
#
# Guarded: ruby_memcheck lives in the optional :valgrind bundler group, so a
# normal `bundle exec rake` (without that group) must not fail to load.
begin
  require "ruby_memcheck"
  require "ruby_memcheck/rspec/rake_task"

  RubyMemcheck.config(
    binary_name: "makiri",
    filter_all_errors: true,    # apply the binary-touch filter to ALL error kinds,
                                # not just leaks (see note above) - drops Ruby's own
                                # GC uninitialised-value noise, keeps mkr_* reports
    valgrind_options: [
      "--num-callers=50",
      "--error-limit=no",
      "--trace-children=yes",   # spec processes may fork
      "--undef-value-errors=yes", # the point of this job (ruby_memcheck defaults to =no)
      # Origin tracking (where an uninitialised value came from) roughly DOUBLES
      # memcheck's run time. It is a diagnostic aid, not a detection capability -
      # memcheck still reports the uninitialised USE without it - so the frequent
      # post-merge push gate turns it off (VALGRIND_TRACK_ORIGINS=no) to run in
      # ~half the time, while the nightly / manual runs keep it on for the backtrace.
      "--track-origins=#{ENV.fetch('VALGRIND_TRACK_ORIGINS', 'yes')}",
      "--leak-check=no",        # leaks are `rake leaks`' job, not this one
    ],
  )

  namespace :spec do
    desc "Run the spec suite under Valgrind memcheck (ruby_memcheck; needs the " \
         ":valgrind bundler group and the valgrind binary)"
    RubyMemcheck::RSpec::RakeTask.new(valgrind: :compile) do |t|
      # Let spec_helper skip :slow examples (fail-closed limit tests whose sheer
      # volume dominates memcheck without adding memory-safety coverage).
      ENV["VALGRIND"] = "1"

      # Optional sharding for CI wall-clock: with VALGRIND_SHARDS=N (>1) the spec
      # files are partitioned into N groups and this run does only group
      # VALGRIND_SHARD_INDEX, so N parallel matrix jobs cover the whole suite in
      # ~1/N the time WITHOUT dropping any coverage. Files are packed
      # largest-first onto the currently-lightest shard (greedy LPT by file size,
      # a proxy for runtime), so the slowest shard - which sets wall-clock - is
      # balanced rather than left to name order. Deterministic, so every shard
      # computes the same partition and takes its own index. Default N=1 = the
      # whole suite (local `rake spec:valgrind`).
      shards = Integer(ENV.fetch("VALGRIND_SHARDS", "1"))
      if shards > 1
        index   = Integer(ENV.fetch("VALGRIND_SHARD_INDEX", "0"))
        buckets = Array.new(shards) { { load: 0, files: [] } }
        # sort by (size desc, name) so the packing is fully deterministic across
        # shards regardless of Array#sort stability.
        Dir["spec/**/*_spec.rb"].sort_by { |f| [-File.size(f), f] }.each do |f|
          b = buckets.min_by { |x| x[:load] }
          b[:files] << f
          b[:load] += File.size(f)
        end
        t.pattern = buckets.fetch(index)[:files]
      end
    end
  end
rescue LoadError
  # ruby_memcheck not installed (optional :valgrind group absent) - skip the task.
end

# The differential against the C build, recorded so it survives the C's removal.
# See spec/differential/run.rb for why this is worth keeping as a fixture.
desc "Compare this build's answers against the recorded C-build baseline"
task diff: :compile do
  sh FileUtils::RUBY, "spec/differential/run.rb"
end

# There is no `diff:record` any more, and there cannot be one. Re-recording
# meant building the C and asking it - and the C is gone, so the baselines under
# spec/differential/baseline/ are now a historical fixture rather than something
# regenerable. That is exactly why they were recorded: `rake diff` still answers
# "does this build agree with what the C answered", which is the one question
# the port had to keep answering after its second implementation disappeared.
#
# A baseline that no longer matches is therefore a finding to investigate, never
# something to refresh. If a difference is genuinely intended (a deliberate
# behaviour change), edit the baseline in the same commit that changes the
# behaviour, so the diff is reviewable as a behaviour change rather than as a
# regenerated blob.

# `rake clean` (from rake-compiler) removes the ext build dir under tmp/,
# including the generated Makefile. The next `rake compile` re-runs extconf,
# so newly-added .c files are picked up - without this, a stale Makefile omits
# new sources and macOS's -undefined dynamic_lookup turns the missing symbols
# into runtime NULL calls. The vendored Lexbor build is deliberately NOT wiped
# here (it is slow to rebuild and rarely changes); use `rake clean:lexbor` for
# a from-scratch Lexbor build.
#
#   rake clean compile     # regenerate ext Makefile + recompile (fast)
#   rake clean:lexbor      # force a full Lexbor rebuild next compile

namespace :clean do
  desc "Remove the vendored Lexbor build/install output (forces a full rebuild)"
  task :lexbor do
    require "fileutils"
    FileUtils.rm_rf("vendor/lexbor/build")
    FileUtils.rm_rf("vendor/lexbor/dist")
  end
end

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
ASAN_ENV_OPTIONS = "detect_leaks=0:detect_container_overflow=0:" \
                   "detect_odr_violation=0:verify_interceptors=0:" \
                   "verify_asan_link_order=0:abort_on_error=1:halt_on_error=1"

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

# The randomised property checks in spec/invariants/. They are not *_spec.rb, so
# `rake spec` skips them; they run here and nightly in CI. Each takes
# [documents] [seed] [html|xml] and is deterministic, so a finding replays.
INVARIANT_CHECKS = [
  ["check_ns_reresolve.rb",       %w[]],
  ["check_import_clone.rb",       %w[]],
  ["check_tree_invariants.rb",    %w[html xml]],
  ["check_index_staleness.rb",    %w[html xml]],
  ["check_serialize_fixpoint.rb", %w[html xml]],
  ["check_text_input.rb",         nil],           # takes no count
].freeze

# [[script, argv], ...] for a given document count.
def invariant_runs(count)
  INVARIANT_CHECKS.flat_map do |script, backends|
    path = "spec/invariants/#{script}"
    next [[path, []]] if backends.nil?
    next [[path, [count.to_s]]] if backends.empty?

    backends.map { |b| [path, [count.to_s, "20260911", b]] }
  end
end

desc "Run the invariant checks (override the document count via INVARIANT_COUNT)"
task invariants: :compile do
  count = (ENV["INVARIANT_COUNT"] || 2000).to_i
  invariant_runs(count).each do |script, argv|
    sh "#{FileUtils::RUBY} -Ilib #{script} #{argv.join(' ')}"
  end
end

desc "Run the invariant checks under AddressSanitizer (the text index holds " \
     "borrowed slices, so staleness there is a memory bug too)"
task "invariants:sanitize" do
  sanitize = ENV["MAKIRI_SANITIZE"] || "address"
  sh({ "MAKIRI_SANITIZE" => sanitize }, "#{FileUtils::RUBY} -S rake clean compile")

  env = {
    "ASAN_OPTIONS"  => ASAN_ENV_OPTIONS,
    # Tells the child it is a sanitizer run. Without it spec_helper cannot know,
    # and its `:slow` exclusion - written for Valgrind, where those examples were
    # measured to dominate - never applied here. That is how a suite which takes
    # 18s plain reached an hour under ASan without anything being wrong.
    "MAKIRI_SANITIZE" => sanitize,
  }.merge(asan_preload_env(sanitize))
  # Instrumented builds are slow; a smaller sweep still exercises every path.
  count = (ENV["INVARIANT_COUNT"] || 500).to_i
  invariant_runs(count).each do |script, argv|
    sh(env, "#{FileUtils::RUBY} -Ilib #{script} #{argv.join(' ')}")
  end
end

desc "Measure coverage of OUR sources (LLVM source-based) over the spec suite. " \
     "Prints an llvm-cov region+branch report (excludes vendored Lexbor and the " \
     "cargo registry) and writes a line-level detail file to tmp/coverage/show.txt."
task :coverage do
  require "fileutils"
  dir = File.expand_path("tmp/coverage")
  FileUtils.rm_rf(dir)
  FileUtils.mkdir_p(dir)

  # Instrument only our sources: MAKIRI_COVERAGE makes extconf pass
  # -Cinstrument-coverage to the crate. Lexbor is built separately by cmake and
  # is not instrumented, and the dependency crates are filtered out of the
  # report below rather than left to dilute it.
  sh({ "MAKIRI_COVERAGE" => "1" }, "#{FileUtils::RUBY} -S rake clean compile")
  # %p -> PID, so any forked spec process gets its own raw profile.
  sh({ "LLVM_PROFILE_FILE" => File.join(dir, "makiri-%p.profraw") }, "#{FileUtils::RUBY} -S rspec")

  profdata = File.join(dir, "makiri.profdata")
  bundle   = "lib/makiri/makiri.bundle"
  ignore   = "(vendor/lexbor|/usr/|/Library/|ruby/|rubygems|[.]cargo/registry|/rustc/)"
  sh "xcrun llvm-profdata merge -sparse #{dir}/*.profraw -o #{profdata}"
  sh "xcrun llvm-cov report #{bundle} -instr-profile=#{profdata} " \
     "-ignore-filename-regex='#{ignore}' -show-branch-summary"
  show = File.join(dir, "show.txt")
  sh "xcrun llvm-cov show #{bundle} -instr-profile=#{profdata} " \
     "-ignore-filename-regex='#{ignore}' -show-branches=count -show-line-counts-or-regions > #{show}"
  puts "\ncoverage line/branch detail: #{show}"
  puts "(coverage build left in place; run `rake clean compile` to restore a normal build)"
end

desc "Like :sanitize but also builds the vendored Lexbor under ASan, so overflows " \
     "INSIDE Lexbor's mraw arena are caught (slow: full Lexbor rebuild). Runs the " \
     "spec suite, or FUZZ_ARGS via the fuzzer when set."
task "sanitize:lexbor" do
  sanitize = ENV["MAKIRI_SANITIZE"] || "address"
  sanitize.include?("address") or
    abort "sanitize:lexbor needs an address build (MAKIRI_SANITIZE must include 'address')"

  # MAKIRI_SANITIZE_LEXBOR makes extconf build Lexbor with -DLEXBOR_BUILD_WITH_ASAN
  # (enabling its mraw poisoning); the build-mode stamp auto-rebuilds Lexbor on the
  # plain<->asan switch, so no manual clean:lexbor is needed before or after.
  build_env = { "MAKIRI_SANITIZE" => sanitize, "MAKIRI_SANITIZE_LEXBOR" => "1" }
  sh(build_env, "#{FileUtils::RUBY} -S rake clean compile")

  env = {
    "ASAN_OPTIONS"  => ASAN_ENV_OPTIONS,
    # Tells the child it is a sanitizer run. Without it spec_helper cannot know,
    # and its `:slow` exclusion - written for Valgrind, where those examples were
    # measured to dominate - never applied here. That is how a suite which takes
    # 18s plain reached an hour under ASan without anything being wrong.
    "MAKIRI_SANITIZE" => sanitize,
  }.merge(asan_preload_env(sanitize))
  if ENV["FUZZ_ARGS"]
    sh(env, "#{FileUtils::RUBY} -Ilib spec/fuzz/run.rb #{ENV['FUZZ_ARGS']}")
  else
    sh(env, "#{FileUtils::RUBY} -S rspec")
  end
end

desc "Run the robustness fuzzer (override options via FUZZ_ARGS)"
task fuzz: :compile do
  sh "#{FileUtils::RUBY} -Ilib spec/fuzz/run.rb #{ENV['FUZZ_ARGS']}"
end

desc "Fuzz the XML parser (hostile/mutated documents; override via FUZZ_ARGS)"
task "fuzz:xml": :compile do
  sh "#{FileUtils::RUBY} -Ilib spec/fuzz/run.rb --target xml #{ENV['FUZZ_ARGS']}"
end

desc "Fuzz the XML mutation surface (random edit sequences + invariants; override via FUZZ_ARGS)"
task "fuzz:mutate": :compile do
  sh "#{FileUtils::RUBY} -Ilib spec/fuzz/run.rb --target mutate #{ENV['FUZZ_ARGS']}"
end

desc "Malloc-leak gate (macOS `leaks`): fails on per-call leak stacks through the ext"
task leaks: :compile do
  # ASan runs with detect_leaks=0 (Ruby/Lexbor are uninstrumented), so plain
  # leaks are otherwise never machine-checked; see script/check_leaks.rb.
  sh "#{FileUtils::RUBY} script/check_leaks.rb"
end
require_relative "tools/pe_exports"

desc "Symbol gate: the built extension must export only Init_makiri and leave no " \
     "Lexbor/Makiri symbol undefined"
task symbols: :compile do
  lib = Dir["lib/makiri/makiri.{bundle,so}"].first or abort "no built extension"
  macos = RbConfig::CONFIG["target_os"] =~ /darwin/
  windows = RbConfig::CONFIG["target_os"] =~ /mingw|mswin/
  # macOS decorates C symbols with a leading underscore; Linux/Windows do not.
  u = macos ? "_" : ""

  # 1. Nothing of ours or Lexbor's may be UNDEFINED. Everything both define is
  #    statically linked in, so an undefined one means the declaration matched
  #    no definition - which macOS does not treat as a link error, because the
  #    extension is linked with -undefined dynamic_lookup. It becomes a NULL
  #    call the first time that function runs. Three inline-only Lexbor
  #    functions reached a shipped build this way once; eight more were
  #    identified while porting glue/ruby_html_node.c, and this is what stops
  #    the next one from getting that far.
  #
  #    On Windows the PE linker already refuses unresolved symbols in the final
  #    DLL, and `nm -u` on a stripped PE image reports no symbols. Use the link
  #    step as the guard there instead of trying to parse the import table.
  bad = if windows
          []
        else
          undef_list = `nm -u #{lib.shellescape}`.lines.map(&:strip)
          # `nm -u` prints bare names on macOS and "U <name>" entries on Linux.
          undef_list.map { |l| l.split.last.to_s }
                    .grep(/\A#{u}(lxb_|lexbor_|mkr_)/)
        end
  unless bad.empty?
    abort "undefined Lexbor/Makiri symbols in #{lib} (they will NULL-call at " \
          "run time):\n  #{bad.uniq.sort.join("\n  ")}"
  end

  # 2. ONLY Init_makiri (and ruby_abi_version, which Ruby reads at require time)
  #    may be EXPORTED. The hazard this addresses is Lexbor's: another
  #    Lexbor-based gem in the same process binding to our differently-versioned
  #    copy and segfaulting - see CLAUDE.md.
  #
  #    This used to assert only "no lxb_ exported", which the flag-based link
  #    made true as a side effect. rustc performs the cdylib link now and passes
  #    its own export list, so the restriction is a post-link trim instead
  #    (extconf.rb) - and a check that asserted less than the claim would have
  #    passed happily while ~220 mkr_* names leaked into the dynamic table.
  #    Assert the claim itself.
  exported = if macos
               `nm -gU #{lib.shellescape}`.lines.grep(/ T /)
             elsif windows
               # nm/objdump on Windows are either the wrong tool, not on PATH,
               # or have output formats that vary by binutils version. Read the
               # PE export directory directly.
               MakiriBuild::PEExports.new(lib).names.map { |name| "#{name}\n" }
             else
               `nm -D --defined-only #{lib.shellescape}`.lines.grep(/ T /)
             end
  allowed = ["#{u}Init_makiri", "#{u}ruby_abi_version"]
  extra = exported.map { |l| l.split.last.to_s }.reject { |s| allowed.include?(s) }
  unless extra.empty?
    abort "#{lib} exports #{extra.size} symbol(s) beyond Init_makiri:\n  " \
          "#{extra.uniq.sort.first(20).join("\n  ")}\n" \
          "The export trim in ext/makiri/rust/extconf.rb did not take effect. " \
          "Do not relax this check - see the note there for the fallback."
  end
  unless exported.any? { |l| l.include?("#{u}Init_makiri") }
    abort "#{lib} does not export Init_makiri - Ruby could not load it."
  end

  puts "symbols: 0 undefined Lexbor/Makiri, exports limited to " \
       "#{allowed.join(' + ')} (#{lib})"
end

desc "OOM-injection gate: rebuild with MAKIRI_ALLOC_INJECT=1 and sweep every core " \
     "allocation site, verifying each failure fails closed (clean raise or " \
     "baseline-identical result, never truncated output)"
task :oom do
  # The hook is compiled in only under MAKIRI_ALLOC_INJECT=1 (zero overhead in
  # a normal build), so this needs its own rebuild; see
  # script/check_alloc_failures.rb for the protocol and the property gated.
  sh({ "MAKIRI_ALLOC_INJECT" => "1" }, "#{FileUtils::RUBY} -S rake clean compile")
  sh "#{FileUtils::RUBY} -Ilib script/check_alloc_failures.rb"
  puts "(injection build left in place; run `rake clean compile` to restore a normal build)"
end

# `rake verify` (CBMC over the Ruby/Lexbor-free C carve-out) is gone with the C
# it proved. Every one of its fifteen harnesses compiled a file under
# ext/makiri/{core,xml,xpath}/, and the three that linked none of them
# (harness_span, harness_spanbuf, harness_hash) proved `static inline` code in
# those same headers. There was nothing left to point CBMC at.
#
# `rake kani` is the successor, and deliberately not a rename: the two tools
# prove different things about different code, and which Kani proof replaces
# which CBMC one - and what changed in the translation - is written down rather
# than assumed. Not all the answers are "the same property".
desc "Kani proofs over the Ruby-free core - the allocator, mkr_buf, UTF-8 " \
     "validate/decode (needs cargo-kani; successor to the C-era CBMC harnesses)"
task :kani do
  # Only the Ruby-free features: anything under glue needs magnus -> rb-sys ->
  # a live Ruby, which Kani cannot build.
  #
  #   rake kani                                   # all six
  #   rake kani HARNESS=accepted_is_utf8          # one
  #   KANI_XML_CHARS_MAX=10 rake kani             # a deeper bound
  #
  # The KANI_*_MAX overrides reach the build through the environment, which `sh`
  # passes on - but they change a `const`, so cargo must see them as a rebuild
  # reason; they are listed here so that is visible rather than folklore.
  # core-utf8 is in the set because it is Ruby-free and its C-ABI proof is
  # gated on it: without the feature that harness silently does not run, which
  # is the failure mode this project keeps finding rather than a saving.
  #
  # `no-c` is in the set because it is what ships. Without it the buffer's
  # ceilings are `extern static`s that Kani, which does not link C, treats as
  # unconstrained values - so the proof would be about a configuration nobody
  # builds. With it they are the consts the extension actually uses.
  argv = ["cargo", "kani", "--no-default-features"]
  harness = ENV["HARNESS"].to_s.strip
  argv += ["--harness", harness] unless harness.empty?
  Dir.chdir("ext/makiri/rust") { sh(*argv) }
end

desc "Run the performance benchmark (Makiri vs Nokogiri reference)"
task bench: :compile do
  # Run outside the bundle so the bench-only gems (nokogiri, benchmark-ips)
  # resolve from system RubyGems without polluting the runtime dependency set.
  Bundler.with_unbundled_env do
    sh "#{FileUtils::RUBY} -Ilib bench/bench.rb"
  end
end

desc "Run the XML reader benchmark (Makiri::XML vs Nokogiri::XML reference)"
task "bench:xml" => :compile do
  Bundler.with_unbundled_env do
    sh "#{FileUtils::RUBY} -Ilib bench/bench_xml.rb"
  end
end

namespace :conformance do
  desc "WHATWG HTML5 parsing conformance: run html5lib-tests through Makiri"
  task html5: :compile do
    sh "#{FileUtils::RUBY} -Ilib spec/conformance/html5lib_runner.rb #{ENV['H5_ARGS']}"
  end

  desc "XPath 1.0 differential conformance vs Nokogiri (libxml2 reference)"
  task xpath: :compile do
    # Like `bench`, run outside the bundle so the bench-only Nokogiri resolves
    # from system RubyGems without entering the runtime dependency set.
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/xpath_diff.rb #{ENV['XPATH_ARGS']}"
    end
  end

  desc "XML XPath 1.0 differential conformance: Makiri::XML vs Nokogiri::XML"
  task xpath_xml: :compile do
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/xml_xpath_diff.rb #{ENV['XPATH_ARGS']}"
    end
  end

  desc "W3C XML Conformance Test Suite: well-formedness through Makiri::XML"
  task xmlconf: :compile do
    # Nokogiri (bench-only) parses the manifests, so run outside the bundle.
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/xmlconf_runner.rb #{ENV['XMLCONF_ARGS']}"
    end
  end

  desc "Property-based XML differential: generated documents, Makiri vs Nokogiri tree"
  task xml_pbt: :compile do
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/xml_pbt_diff.rb #{ENV['PBT_ARGS']}"
    end
  end

  desc "CSS Selectors differential conformance vs Nokogiri::HTML5"
  task css: :compile do
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/css_diff.rb #{ENV['CSS_ARGS']}"
    end
  end

  desc "XML CSS-selector differential conformance: Makiri::XML vs Nokogiri::XML"
  task css_xml: :compile do
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/xml_css_diff.rb #{ENV['CSS_XML_ARGS']}"
    end
  end

  desc "XML Builder differential conformance: Makiri::XML::Builder vs Nokogiri::XML::Builder"
  task builder: :compile do
    Bundler.with_unbundled_env do
      sh "#{FileUtils::RUBY} -Ilib spec/conformance/builder_diff.rb #{ENV['BUILDER_ARGS']}"
    end
  end
end

desc "Run all conformance suites"
task conformance: %w[conformance:html5 conformance:xpath conformance:css
                     conformance:xmlconf conformance:xpath_xml conformance:css_xml
                     conformance:builder]

namespace :fuzz do
  # Run the fuzzer under the sanitizer. Toggles (all via env):
  #   FAST=1        run the surfaces NON-isolated (one process, no fork-per-query).
  #                 Far higher throughput; ASan still aborts on a memory error
  #                 (halt_on_error). The default (isolated) is the complete net:
  #                 it also survives + attributes a genuine segfault and catches a
  #                 hang via the per-query timeout, at much lower throughput.
  #   SKIP_BUILD=1  reuse the current build instead of rebuilding (refuses to run
  #                 if it is not a sanitizer build, so you never fuzz a plain ext).
  #   FUZZ_TIME=N   seconds per surface (default 90).
  #   FUZZ_ARGS=... run a single custom invocation instead of the three surfaces.
  desc "Run the fuzzer under AddressSanitizer (FAST=1 non-isolated, SKIP_BUILD=1 reuse build)"
  task :sanitize do
    sanitize = ENV["MAKIRI_SANITIZE"] || "address"
    if %w[1 true yes].include?(ENV["SKIP_BUILD"].to_s.downcase)
      puts "fuzz:sanitize: reusing the existing sanitizer build (SKIP_BUILD)"
    else
      sh({ "MAKIRI_SANITIZE" => sanitize }, "#{FileUtils::RUBY} -S rake clean compile")
    end
    # SKIP_BUILD reuses whatever is on disk, so this is the path most likely to
    # fuzz an uninstrumented build (see ext_asan_instrumented?).
    assert_sanitized!("fuzz:sanitize", sanitize)

    env = {
      "ASAN_OPTIONS"  => ASAN_ENV_OPTIONS,
      "UBSAN_OPTIONS" => "print_stacktrace=1:halt_on_error=1",
    }.merge(asan_preload_env(sanitize))
    if ENV["FUZZ_ARGS"]
      sh(env, "#{FileUtils::RUBY} -Ilib spec/fuzz/run.rb #{ENV['FUZZ_ARGS']}")
    else
      iso  = %w[1 true yes].include?(ENV["ISOLATED"].to_s.downcase) ? "--isolated" : ""
      secs = ENV["FUZZ_TIME"] || "90"
      # Cover every surface under the sanitizer: the query engine (XPath/CSS over
      # parsed fixtures), the XML parser (hostile documents), and the XML mutation
      # surface (random edit sequences + invariants).
      ["", "--target xml", "--target mutate", "--target xmlcss"].each do |surface|
        sh(env, "#{FileUtils::RUBY} -Ilib spec/fuzz/run.rb #{surface} #{iso} --time #{secs}".squeeze(" ").strip)
      end
    end
  end

  # Coverage-guided libFuzzer harnesses for the Ruby-free surfaces (the XML
  # reader, the XPath front end, and XPath over a parsed XML tree). They are
  # standalone binaries, so they run without the Ruby interpreter, and they
  # complement the Ruby-based robustness fuzzer by providing coverage feedback
  # and 2-3 orders of magnitude faster execution for the engine core.
  #
  # Nothing here depends on the built extension: cargo-fuzz builds the crate
  # itself, with its own instrumentation. It does need the vendored Lexbor
  # HEADERS (the crate's build.rs generates the layout from them), which
  # `rake compile` produces - hence the dependency, which is about Lexbor rather
  # than about the bundle.
  FUZZ_TARGETS = %w[xml xpath xml_xpath].freeze

  desc "Build the cargo-fuzz harnesses (requires cargo-fuzz and a nightly toolchain)"
  task :libfuzzer_build => :compile do
    cargo_fuzz_available? or
      abort "fuzz:libfuzzer_build: cargo-fuzz is not installed " \
            "(`cargo install cargo-fuzz`; it needs a nightly toolchain)."
    Dir.chdir("ext/makiri/rust/fuzz") { sh "cargo", "fuzz", "build" }
  end

  desc "Run the cargo-fuzz coverage-guided harnesses (default: 60s per target)"
  task :libfuzzer => :libfuzzer_build do
    time = ENV["FUZZ_TIME"] || "60"
    Dir.chdir("ext/makiri/rust/fuzz") do
      FUZZ_TARGETS.each do |target|
        sh "cargo", "fuzz", "run", target, "--",
           "-max_total_time=#{time}", "-max_len=4096"
      end
    end
  end
end

desc "Show code statistics"
task :stats do
  sh "tokei lib ext spec script --exclude tmp --exclude vendor --exclude docs"
end
