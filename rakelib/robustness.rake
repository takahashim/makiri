# frozen_string_literal: true

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

desc "Run the invariant checks under AddressSanitizer (memory errors in the " \
     "arenas, their indexes and edits)"
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
