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

Rake::ExtensionTask.new("makiri", GEMSPEC) do |ext|
  ext.lib_dir       = "lib/makiri"
  ext.ext_dir       = "ext/makiri"
  ext.source_pattern = "**/*.{c,h}"
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

desc "Check that the port configuration agrees with itself (table, features, CI legs)"
task :ports do
  sh FileUtils::RUBY, "script/check_port_table.rb"
end

namespace :security do
  desc "Run mechanical C safety lint over ext/makiri"
  task :clint do
    sh FileUtils::RUBY, "script/check_c_safety.rb", *Shellwords.split(ENV.fetch("C_LINT_ARGS", ""))
  end
end

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

# Locate the AddressSanitizer runtime shared library for the active compiler, so
# it can be preloaded ahead of Ruby (sanitized extensions dlopen'd late
# otherwise abort with "ASan runtime does not come first").
def asan_runtime_path
  cc = RbConfig::CONFIG["CC"] || "cc"
  names =
    if RbConfig::CONFIG["target_os"] =~ /darwin/
      %w[libclang_rt.asan_osx_dynamic.dylib]
    else
      arch = RUBY_PLATFORM[/x86_64|aarch64|arm64/] || "x86_64"
      ["libasan.so", "libclang_rt.asan-#{arch}.so", "libclang_rt.asan.so"]
    end
  names.each do |name|
    path = `#{cc} -print-file-name=#{name} 2>/dev/null`.strip
    return path if path != name && !path.empty? && File.exist?(path)
  end
  nil
end

def libfuzzer_available?
  cxx = ENV["CXX"].to_s.empty? ? "clang++" : ENV["CXX"]
  Dir.mktmpdir("makiri-libfuzzer-check") do |dir|
    src = File.join(dir, "check.cc")
    exe = File.join(dir, "check")
    File.write(src, "extern \"C\" int LLVMFuzzerTestOneInput(const unsigned char*, unsigned long){return 0;}\n")
    return system(cxx, "-fsanitize=fuzzer,address,undefined", src, "-o", exe,
                  out: File::NULL, err: File::NULL)
  end
end

# The compiled extension, and whether it carries sanitizer instrumentation, so
# `fuzz:sanitize SKIP_BUILD=1` can refuse to run a plain (non-ASan) build.
def ext_bundle_path
  Dir["lib/makiri/makiri.{bundle,so}"].first
end

def ext_sanitized?
  bundle = ext_bundle_path or return false
  !(`nm "#{bundle}" 2>/dev/null` =~ /asan|ubsan/i).nil?
end

# Is the Rust crate itself ASan-instrumented? nil when this build has no Rust.
#
# ext_sanitized? cannot answer this: the bundle references __asan_* as soon as
# ANY translation unit is instrumented, and the C sources always are - so it
# reports "sanitized" for a build whose entire Rust half is plain. That is the
# exact failure this check exists to prevent, and it is not hypothetical: until
# 2026-09-12 extconf passed the sanitizer to $CFLAGS only, so every sanitizer
# run since the first glue port covered less than it appeared to, silently.
# Instrumented code calls __asan_report_* on a failing access; uninstrumented
# code has no such reference. Look inside the crate's own archive.
# The C sources this configuration replaces, as paths relative to ext/makiri -
# the form verify/Makefile spells them in. Read from the one table that decides
# it (extconf's RUST_PORTS) rather than restated.
def rust_replaced_sources
  require_relative "ext/makiri/rust_ports"
  RustPorts.replaced_sources
end

# Is this build supposed to contain Rust at all?
def rust_build?
  ENV["MAKIRI_RUST"].to_s.strip == "all" ||
    ENV.keys.any? { |k| k.start_with?("MAKIRI_RUST_") }
end

# Is the Rust crate ASan-instrumented? Raises rather than guessing when it
# cannot tell.
#
# The first version returned nil for BOTH "no Rust in this build" and "could not
# find the archive", and the caller treated nil as pass. A moved tmp/ layout, a
# second Ruby version, a clean that put the archive elsewhere - any of those
# would have made the check silently vacuous, which is the third time in this
# work that a check has looked like it was checking. The two states are separate
# now, and "should be there, is not" aborts.
def rust_asan_instrumented?
  archives = Dir.glob("tmp/**/rust-target/**/libmakiri_rs.a")
  if archives.empty?
    return nil unless rust_build?

    abort "sanitize: a Rust build was asked for but no libmakiri_rs.a was found under " \
          "tmp/ - the Rust half cannot be checked, so this run would cover " \
          "less than it claims. (Looked for tmp/**/rust-target/**/libmakiri_rs.a.)"
  end
  # Every one of them, not the first: more than one appears across Ruby versions
  # and platforms, and an uninstrumented straggler is exactly what this catches.
  archives.all? { |a| `nm -u "#{a}" 2>/dev/null` =~ /__asan_report/ }
end

# Abort unless BOTH halves are instrumented. Called by the sanitizer tasks after
# the build, so a green run always means what it looks like it means.
#
# One entry point rather than two: `ext_sanitized?` answers for the C and
# `rust_asan_instrumented?` for the Rust, and a caller that remembers one and
# forgets the other gets exactly the half-covered run this is here to prevent.
def assert_sanitized!(task, sanitize)
  return unless sanitize.include?("address")

  unless ext_sanitized?
    abort "#{task}: lib/makiri is not a sanitizer build."
  end
  assert_rust_asan!(task)
end

# The Rust half. Prefer `assert_sanitized!`.
def assert_rust_asan!(task)
  case rust_asan_instrumented?
  when nil then nil # a C-only build; there is no Rust half to instrument
  when true
    puts "#{task}: ASan covers C and Rust; " \
         "UBSan covers the C sources only (Rust has no UBSan)"
  else
    abort "#{task}: the Rust crate is NOT ASan-instrumented, so this run would " \
          "cover only the C half and still come out green. Rebuild without " \
          "SKIP_BUILD (extconf passes -Zsanitizer=address when ASan is on)."
  end
end

desc "Build the extension with sanitizers (MAKIRI_SANITIZE, default " \
     "address,undefined) and run the spec suite under them. With MAKIRI_RUST_* " \
     "set, ASan also covers the Rust crate (needs nightly); UBSan never does"
task :sanitize do
  sanitize = ENV["MAKIRI_SANITIZE"] || "address,undefined"
  sh({ "MAKIRI_SANITIZE" => sanitize }, "#{FileUtils::RUBY} -S rake clean compile")

  # What this run does and does not cover, stated up front: a sanitizer run that
  # quietly skips half the extension is worse than no run, and with the port in
  # progress "half" is literal. extconf builds the crate with -Zsanitizer=address
  # when ASan is on (it aborts if nightly is missing rather than silently
  # building it plain), but Rust has no UBSan at all - rustc's -Zsanitizer takes
  # address/cfi/dataflow/hwaddress/kcfi/kernel-address/leak/memory/memtag/
  # safestack/shadow-call-stack/thread/realtime, and `undefined` is not one of
  # them. That is permanent, not a gap waiting to be closed.
  assert_sanitized!("sanitize", sanitize)

  env = {
    # LeakSanitizer would flag Ruby's intentional caches; the interpreter is not
    # instrumented, so silence the noise and keep real heap/UB findings fatal.
    "ASAN_OPTIONS"  => "detect_leaks=0:detect_container_overflow=0:" \
                       "detect_odr_violation=0:abort_on_error=1:halt_on_error=1",
    "UBSAN_OPTIONS" => "print_stacktrace=1:halt_on_error=1",
  }
  if sanitize.include?("address")
    runtime = asan_runtime_path or
      abort "sanitize: could not locate the ASan runtime for #{RbConfig::CONFIG['CC']}"
    preload = RbConfig::CONFIG["target_os"] =~ /darwin/ ? "DYLD_INSERT_LIBRARIES" : "LD_PRELOAD"
    env[preload] = runtime
    puts "sanitize: preloading #{runtime} via #{preload}"
  end

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

desc "Run the invariant checks under AddressSanitizer + UBSan (the text index " \
     "holds borrowed slices, so staleness there is a memory bug too)"
task "invariants:sanitize" do
  sanitize = ENV["MAKIRI_SANITIZE"] || "address,undefined"
  sh({ "MAKIRI_SANITIZE" => sanitize }, "#{FileUtils::RUBY} -S rake clean compile")

  env = {
    "ASAN_OPTIONS"  => "detect_leaks=0:detect_container_overflow=0:" \
                       "detect_odr_violation=0:abort_on_error=1:halt_on_error=1",
    "UBSAN_OPTIONS" => "print_stacktrace=1:halt_on_error=1",
  }
  if sanitize.include?("address")
    runtime = asan_runtime_path or
      abort "invariants:sanitize: could not locate the ASan runtime for #{RbConfig::CONFIG['CC']}"
    preload = RbConfig::CONFIG["target_os"] =~ /darwin/ ? "DYLD_INSERT_LIBRARIES" : "LD_PRELOAD"
    env[preload] = runtime
    puts "invariants:sanitize: preloading #{runtime} via #{preload}"
  end

  # Instrumented builds are slow; a smaller sweep still exercises every path.
  count = (ENV["INVARIANT_COUNT"] || 500).to_i
  invariant_runs(count).each do |script, argv|
    sh(env, "#{FileUtils::RUBY} -Ilib #{script} #{argv.join(' ')}")
  end
end

desc "Measure C coverage of OUR sources (clang source-based) over the spec suite. " \
     "Prints an llvm-cov region+branch report (excludes vendored Lexbor) and writes " \
     "a line-level detail file to tmp/coverage/show.txt."
task :coverage do
  require "fileutils"
  dir = File.expand_path("tmp/coverage")
  FileUtils.rm_rf(dir)
  FileUtils.mkdir_p(dir)

  # Instrument only our sources (Lexbor is built separately, uninstrumented).
  sh({ "MAKIRI_COVERAGE" => "1" }, "#{FileUtils::RUBY} -S rake clean compile")
  # %p -> PID, so any forked spec process gets its own raw profile.
  sh({ "LLVM_PROFILE_FILE" => File.join(dir, "makiri-%p.profraw") }, "#{FileUtils::RUBY} -S rspec")

  profdata = File.join(dir, "makiri.profdata")
  bundle   = "lib/makiri/makiri.bundle"
  ignore   = "(vendor/lexbor|/usr/|/Library/|ruby/|rubygems)"
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
  sanitize = ENV["MAKIRI_SANITIZE"] || "address,undefined"
  sanitize.include?("address") or
    abort "sanitize:lexbor needs an address build (MAKIRI_SANITIZE must include 'address')"

  # MAKIRI_SANITIZE_LEXBOR makes extconf build Lexbor with -DLEXBOR_BUILD_WITH_ASAN
  # (enabling its mraw poisoning); the build-mode stamp auto-rebuilds Lexbor on the
  # plain<->asan switch, so no manual clean:lexbor is needed before or after.
  build_env = { "MAKIRI_SANITIZE" => sanitize, "MAKIRI_SANITIZE_LEXBOR" => "1" }
  sh(build_env, "#{FileUtils::RUBY} -S rake clean compile")

  env = {
    "ASAN_OPTIONS"  => "detect_leaks=0:detect_container_overflow=0:" \
                       "detect_odr_violation=0:abort_on_error=1:halt_on_error=1",
    "UBSAN_OPTIONS" => "print_stacktrace=1:halt_on_error=1",
  }
  runtime = asan_runtime_path or
    abort "sanitize:lexbor: could not locate the ASan runtime for #{RbConfig::CONFIG['CC']}"
  preload = RbConfig::CONFIG["target_os"] =~ /darwin/ ? "DYLD_INSERT_LIBRARIES" : "LD_PRELOAD"
  env[preload] = runtime
  puts "sanitize:lexbor: preloading #{runtime} via #{preload}"

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

desc "Symbol gate: the built extension must export only Init_makiri and leave no " \
     "Lexbor/Makiri symbol undefined"
task symbols: :compile do
  lib = Dir["lib/makiri/makiri.{bundle,so}"].first or abort "no built extension"
  macos = RbConfig::CONFIG["target_os"] =~ /darwin/
  # macOS decorates C symbols with a leading underscore; Linux does not.
  u = macos ? "_" : ""

  # 1. Nothing of ours or Lexbor's may be UNDEFINED. Everything both define is
  #    statically linked in, so an undefined one means the declaration matched
  #    no definition - which macOS does not treat as a link error, because the
  #    extension is linked with -undefined dynamic_lookup. It becomes a NULL
  #    call the first time that function runs. Three inline-only Lexbor
  #    functions reached a shipped build this way once; eight more were
  #    identified while porting glue/ruby_html_node.c, and this is what stops
  #    the next one from getting that far.
  undef_list = `nm -u #{lib.shellescape}`.lines.map(&:strip)
  # `nm -u` prints bare names on macOS and "U <name>" entries on Linux.
  bad = undef_list.map { |l| l.split.last.to_s }
                  .grep(/\A#{u}(lxb_|lexbor_|mkr_)/)
  unless bad.empty?
    abort "undefined Lexbor/Makiri symbols in #{lib} (they will NULL-call at " \
          "run time):\n  #{bad.uniq.sort.join("\n  ")}"
  end

  # 2. Nothing of Lexbor's may be EXPORTED either - see CLAUDE.md: another
  #    Lexbor-based gem in the same process would bind to our different version.
  exported = if macos
               `nm -gU #{lib.shellescape}`.lines.grep(/ T _lxb_/)
             else
               `nm -D --defined-only #{lib.shellescape}`.lines.grep(/ T lxb_/)
             end
  unless exported.empty?
    abort "#{lib} re-exports #{exported.size} Lexbor symbols; check the " \
          "-exported_symbol / --exclude-libs link flags in extconf.rb"
  end

  puts "symbols: 0 undefined Lexbor/Makiri, 0 exported Lexbor (#{lib})"
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

desc "CBMC proofs over the Ruby/Lexbor-free carve-out (core + XML + XPath front; " \
     "needs cbmc; see docs/formal_verification.ja.md and verify/Makefile)"
task :verify do
  # The 10 CBMC proofs are independent processes with a lopsided cost
  # distribution (a few ~1-2min solves dominate, the rest are trivial), so run
  # them in parallel: wall-clock drops toward the single slowest harness with no
  # loss of coverage. Cap -j at the CPU count (Etc.nprocessors; the runner has
  # RAM for that many concurrent core-set solves). smoke/selftest are quick and
  # come first as ordered goals.
  require "etc"
  jobs = Integer(ENV.fetch("VERIFY_JOBS", Etc.nprocessors))
  sh "make", "-C", "verify", "-j#{jobs}", "smoke", "selftest", "cbmc"

  # Two of those proofs are over C sources a MAKIRI_RUST_* build does not
  # compile, so a green run says nothing about that build. They are not removed
  # - the C build is still what a default `gem install` gets, and they are real
  # for it - but the result must not read as coverage it does not have. `rake
  # kani` is what covers the Rust engine (notes/rust_port_remaining.ja.md §4).
  next unless rust_build?

  # Which proofs are orphaned is derived, not named: each CBMC target lists the
  # C sources it compiles, and this configuration lists the ones it replaces.
  # Naming them in prose was a second copy of that knowledge, and it would have
  # gone quietly stale the next time a file was ported.
  makefile = File.read("verify/Makefile")
  replaced = rust_replaced_sources
  orphaned = makefile.scan(/^cbmc-([a-z0-9-]+):\n((?:\t.*\n)+)/).filter_map do |name, body|
    name if replaced.any? { |src| body.include?(src) }
  end
  next if orphaned.empty?

  warn <<~ORPHANED

    verify: NOTE - #{orphaned.map { |n| "cbmc-#{n}" }.join(", ")} proved C sources
      that THIS configuration replaces with Rust. For the build you just asked
      for they are legacy-c-proof: true of the C, silent about the Rust. Run
      `bundle exec rake kani` for that half.
  ORPHANED
end

# The Rust-port configuration. `MAKIRI_RUST=all` is what CI and the container
# scripts set - extconf expands it from ext/makiri/rust_ports.rb, so nothing
# outside that file enumerates the flags. The one thing still needing a list is
# `cargo clippy --features`, which is what this is for.
#
# There was a `rust:check` beside it that validated the table. It is gone, and
# deliberately: both halves of what it checked are enforced by something that
# runs on every build. A srcs path that does not exist aborts in extconf
# (RustPorts.check_paths!, called before anything else, so even a plain C build
# fails), and a cargo feature Cargo.toml does not declare fails the cargo
# invocation itself ("the package 'makiri_rs' does not contain this feature").
# Both verified by probe. A task that looks like a guard but guards nothing is
# worse than no task - it invites the next person to trust it.
namespace :rust do
  desc "Print the cargo feature list for MAKIRI_RUST=all (or FEATURES, passed through)"
  task :features do
    require_relative "ext/makiri/rust_ports"
    given = ENV["FEATURES"].to_s.strip
    puts given.empty? ? RustPorts.features(RustPorts.enabled("MAKIRI_RUST" => "all")).join(",") : given
  end
end

# Kani is to the Rust half what CBMC is to the C half. It is a separate task
# rather than part of `verify` because it needs a different tool, and because
# the two do NOT cover the same code: a proof over ext/makiri/xml/*.c says
# nothing about a build where those files are replaced. Which proof replaces
# which, and what changed in the translation, is written down in
# notes/rust_port_remaining.ja.md - the answers are not all "the same property".
desc "Kani proofs over the Rust engine (needs cargo-kani; see notes/rust_port_remaining.ja.md)"
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
  argv = ["cargo", "kani", "--features", "xml,xpath,core-utf8,core-buf,core-alloc"]
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
    sanitize = ENV["MAKIRI_SANITIZE"] || "address,undefined"
    if %w[1 true yes].include?(ENV["SKIP_BUILD"].to_s.downcase)
      puts "fuzz:sanitize: reusing the existing sanitizer build (SKIP_BUILD)"
    else
      sh({ "MAKIRI_SANITIZE" => sanitize }, "#{FileUtils::RUBY} -S rake clean compile")
    end
    # SKIP_BUILD reuses whatever is on disk, so this is the path most likely to
    # fuzz a half-instrumented build (see rust_asan_instrumented?).
    assert_sanitized!("fuzz:sanitize", sanitize)

    env = {
      "ASAN_OPTIONS"  => "detect_leaks=0:detect_container_overflow=0:" \
                         "detect_odr_violation=0:abort_on_error=1:halt_on_error=1",
      "UBSAN_OPTIONS" => "print_stacktrace=1:halt_on_error=1",
    }
    if sanitize.include?("address")
      runtime = asan_runtime_path or
        abort "fuzz:sanitize: could not locate the ASan runtime"
      preload = RbConfig::CONFIG["target_os"] =~ /darwin/ ? "DYLD_INSERT_LIBRARIES" : "LD_PRELOAD"
      env[preload] = runtime
    end

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

  # Coverage-guided libFuzzer harnesses for the pure-C surfaces (XML parser and
  # XPath compile+eval).  These are Ruby-free standalone binaries, so they run
  # directly under clang's libFuzzer driver without the Ruby interpreter.
  # They complement the Ruby-based robustness fuzzer by providing coverage
  # feedback and 2-3 orders of magnitude faster execution for the C core.
  desc "Build the libFuzzer harnesses (requires clang with libFuzzer support)"
  task :libfuzzer_build => :compile do
    libfuzzer_available? or
      abort "fuzz:libfuzzer_build: #{ENV['CXX'] || 'clang++'} cannot link libFuzzer. " \
            "Install an LLVM clang with libFuzzer support and run with " \
            "CLANG=/path/to/clang CXX=/path/to/clang++."
    Dir.chdir("ext/makiri/fuzz") do
      sh "make clean"
      sh "make all"
    end
  end

  desc "Run the libFuzzer coverage-guided harnesses (default: 60s per target)"
  task :libfuzzer => :libfuzzer_build do
    time = ENV["FUZZ_TIME"] || "60"
    Dir.chdir("ext/makiri/fuzz") do
      sh "mkdir -p corpus/xml corpus/xpath"
      sh "./xml_fuzz -max_total_time=#{time} -max_len=4096 corpus/xml"
      sh "./xpath_fuzz -max_total_time=#{time} -max_len=4096 corpus/xpath"
    end
  end
end

desc "Show code statistics"
task :stats do
  sh "tokei lib ext spec script --exclude tmp --exclude vendor --exclude docs"
end
