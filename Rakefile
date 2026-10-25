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
# (the link arguments, the export trim) already are.
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
# at all, and its default pattern is for C. Limit it to the crate's own sources
# so the cargo-fuzz crate and examples are not staged as extension sources.
Rake::ExtensionTask.new("makiri", GEMSPEC) do |ext|
  ext.lib_dir        = "lib/makiri"
  ext.ext_dir        = "ext/makiri/rust"
  ext.source_pattern = "src/**/*.{rs,toml}"
end

RSpec::Core::RakeTask.new(:spec)

task default: %i[compile spec]

namespace :unsafe do
  desc "Check the Rust safe-module and reviewed-global boundaries"
  task :boundaries do
    sh FileUtils::RUBY, "script/check_unsafe_boundaries.rb"
  end

  # Transcribes the unsafe-island counts and the forbid list into the script,
  # so a hand count cannot disagree with the one the check does. It does not
  # touch the tables that pin a boundary decision (`rb_sys::`, `static mut`,
  # the raising C API) - those still fail until a person raises them.
  desc "Rewrite the recorded unsafe islands and forbid list from what is there"
  task :fix do
    sh FileUtils::RUBY, "script/check_unsafe_boundaries.rb", "--fix"
  end
end

# The extension's dependencies against ext/makiri/rust/deny.toml: RustSec
# advisories and yanked releases, licenses, sources (CI: security.yml, nightly
# too). Needs cargo-deny (`cargo install cargo-deny --locked`).
desc "Check the extension's dependencies with cargo-deny (advisories, licenses, sources)"
task :deny do
  Dir.chdir("ext/makiri/rust") { sh "cargo", "deny", "check" }
end

# Verus over what Kani proves bounded (ext/makiri/rust/verus/): the growth
# arithmetic, copied from falloc, and RawVec over its Block's contract. The
# copies are checked
# against the source first - every line only Verus needs ends in `// [verus]`,
# and the rest of the body must be the source's, token for token - so a
# change to `grow_capacity` fails here until the proof is redone. VERUS names
# the binary (else `verus` on PATH); CI: verify.yml, at VERUS_VERSION.
VERUS_VERSION = "0.2026.10.04.426d8b0"
VERUS_COPIES = [
  ["ext/makiri/rust/src/falloc/mod.rs", "ext/makiri/rust/verus/raw_vec.rs", "grow_capacity"],
  *%w[as_slice reserve grow_to push].map do |name|
    ["ext/makiri/rust/src/raw_vec/mod.rs", "ext/makiri/rust/verus/raw_vec.rs", name]
  end,
  *%w[capacity_for probe lookup PtrTable::with_keys PtrTable::insert PtrTable::get
      PtrMap::clear PtrMap::get PtrMap::insert PtrMap::set PtrMap::grow].map do |name|
    ["ext/makiri/rust/src/ptr_table.rs", "ext/makiri/rust/verus/ptr_table.rs", name]
  end
].freeze
# The files `rake verus` verifies.
VERUS_PROOFS = %w[raw_vec.rs ptr_table.rs].freeze

# The body of `fn name` in `src` - `Type::name` for the one in `impl ... Type`
# - as tokens: lines marked `// [verus]` and
# comments (line and block) dropped, from the brace that opens the body to the one closing it,
# and a trailing comma before a closing bracket dropped too - `cargo fmt`
# adds one when it breaks a call across lines, and it changes nothing.
def verus_body_tokens(src, name)
  text = src.lines.reject { |l| l.rstrip.end_with?("// [verus]") }
            .map { |l| l.sub(%r{//.*$}, "") }.join.gsub(%r{/\*.*?\*/}m, " ")
  type, name = name.include?("::") ? name.split("::", 2) : [nil, name]
  from = type ? (text.index(/^impl\b[^{]*\b#{Regexp.escape(type)}\s*</) or return nil) : 0
  at = text.index(/\bfn\s+#{Regexp.escape(name)}\b/, from) or return nil
  open = text.index("{", at) or return nil
  depth = 0
  close = (open...text.length).find do |i|
    depth += 1 if text[i] == "{"
    depth -= 1 if text[i] == "}"
    depth.zero?
  end
  return nil unless close

  toks = text[open..close].scan(/\w+|[^\s\w]/)
  toks.each_index.reject { |i| toks[i] == "," && ")]}".include?(toks[i + 1].to_s) }.map { |i| toks[i] }
end

namespace :verus do
  desc "Check that the Verus copies match the source they were copied from"
  task :sync do
    VERUS_COPIES.each do |source, copy, name|
      want = verus_body_tokens(File.read(source), name) or abort "verus: no fn #{name} in #{source}"
      got = verus_body_tokens(File.read(copy), name) or abort "verus: no fn #{name} in #{copy}"
      next if want == got

      abort "verus: #{copy}'s #{name} is not #{source}'s - redo the copy and its proof"
    end
    puts "verus: the copies match their sources"
  end
end

# What each proof takes on trust, pinned: a `#[verifier::external_body]` (a
# body Verus does not read, its spec assumed), an `assume_specification`
# (std's spec assumed), an `assume(` or `admit(`. Each one is named in its
# file's module doc; a new one is a new axiom, and this count moves with it on
# purpose. The canaries may hold none.
VERUS_TRUSTED = { "raw_vec.rs" => 5, "ptr_table.rs" => 3 }.freeze
VERUS_TRUST = /#\[verifier::external(?:_body)?\]|\bassume_specification\b|\b(?:assume|admit)\(/

def verus_trusted_count(src)
  src.gsub(%r{/\*.*?\*/}m, "").lines.map { |l| l.sub(%r{//.*$}, "") }.join.scan(VERUS_TRUST).length
end

# A proof is vacuous when what it assumes - an axiom, a precondition, a fact
# met along the way - contradicts itself: then every claim after it verifies.
# Verus has no `cover`, so `rake verus` checks the same way the dylint
# self-test does, by a check that must FAIL:
#
# - each `// [canary]` line of a proof marks a point the proved code reaches;
#   for each, the proof is verified again with an `assert(false)` there, which
#   must be the one thing that fails. (One variant per mark: a failed assert is
#   assumed after, so two in one run would hide the second.) This is what sees
#   a contradiction inside a body, which the body's callers cannot - they see
#   only its `ensures`;
# - verus/canaries/<proof> adds `canary_` functions that call the proved code
#   from outside and `assert(false)` on each outcome, so a precondition no
#   caller can meet, or `ensures` that contradict, is caught too.
def verus_assertions_failing(verus, path)
  out, = Open3.capture2e(verus, path)
  lines = out.scan(/^error: assertion failed\s*\n\s*--> [^\n]*:(\d+):/).flatten.map(&:to_i)
  errors = out[/verification results:: \d+ verified, (\d+) errors?/, 1]
  [lines, errors&.to_i, out]
end

def verus_canaries(verus, proof)
  dir = File.join(__dir__, "tmp/verus_canaries")
  FileUtils.mkdir_p(dir)
  src = File.read(proof)
  lines = src.lines
  marks = lines.each_index.select { |i| lines[i].strip == "// [canary]" }
  abort "verus: #{proof} has no `// [canary]` marks" if marks.empty?

  jobs = marks.map do |i|
    variant = lines.dup
    variant[i] = lines[i].sub("// [canary]", "assert(false); // [canary]")
    path = File.join(dir, "#{File.basename(proof, '.rs')}_mark#{i + 1}.rs")
    File.write(path, variant.join)
    [path, [i + 1]]
  end

  canary = File.read("canaries/#{proof}")
  at = src.rindex("} // verus!") or abort "verus: #{proof} has no `} // verus!` to put the canaries before"
  first = src[0...at].count("\n") + 1
  outside = File.join(dir, proof)
  File.write(outside, src[0...at] + canary + src[at..])
  want = (canary.lines.each_index.select { |i| canary.lines[i].include?("assert(false)") }).map { |i| first + i }
  jobs << [outside, want]

  queue = Queue.new
  jobs.each { |j| queue << j }
  failures = Queue.new
  Array.new(4) do
    Thread.new do
      while (job = (queue.pop(true) rescue nil))
        path, expected = job
        got, errors, out = verus_assertions_failing(verus, path)
        failures << [path, expected, got, out] unless got.sort == expected.sort && errors == expected.length
      end
    end
  end.each(&:join)
  return puts("verus: #{proof}: #{marks.length} marks and #{want.length} canaries, each failing where it must") if failures.empty?

  failures.size.times do
    path, expected, got, out = failures.pop
    puts out
    puts "verus: #{path}: expected assert(false) to fail at lines #{expected}, failed at #{got}"
  end
  abort "verus: #{proof}: a canary verified - an assumption the proof rests on proves anything there"
end

desc "Verify the Verus proofs (ext/makiri/rust/verus/; needs Verus #{VERUS_VERSION})"
task verus: "verus:sync" do
  require "open3"
  verus = ENV.fetch("VERUS", "verus")
  Dir.chdir("ext/makiri/rust/verus") do
    trusted = VERUS_PROOFS.to_h { |f| [f, verus_trusted_count(File.read(f))] }
    abort "verus: what the proofs take on trust changed: #{trusted} (recorded #{VERUS_TRUSTED})" \
      unless trusted == VERUS_TRUSTED
    untrusting = VERUS_PROOFS.reject { |f| verus_trusted_count(File.read("canaries/#{f}")).zero? }
    abort "verus: canaries may assume nothing: #{untrusting}" unless untrusting.empty?

    VERUS_PROOFS.each do |f|
      sh verus, f
      verus_canaries(verus, f)
    end
  end
  puts "verus: every proof verified, its canaries failed as they must; trusted #{VERUS_TRUSTED}"
end

# Miri over the Ruby-free modules that hold `unsafe`, and the buffer built on
# them: their tests on Miri's interpreter, which reports undefined behaviour a
# run on hardware passes over (CI: verify.yml). Needs the nightly toolchain
# with the `miri` component. MIRI_FILTERS replaces the module list.
MIRI_FILTERS = %w[raw_vec:: utf8_copy:: falloc:: cbuf:: ptr_table::].freeze # raw_vec:: covers raw_vec::block
desc "Run the Ruby-free unsafe modules' tests under Miri (nightly)"
task :miri do
  filters = ENV["MIRI_FILTERS"] ? ENV["MIRI_FILTERS"].split(",") : MIRI_FILTERS
  env = { "MIRIFLAGS" => ENV.fetch("MIRIFLAGS", "-Zmiri-strict-provenance") }
  Dir.chdir("ext/makiri/rust") do
    sh env, "cargo", "+nightly", "miri", "test", "--no-default-features", "--lib", "--", *filters
  end
end

# The places that must not unwind (`src/no_unwind.rs`): every `extern "C"`
# function of ours, and the `Drop`s the GC's free callback runs. An unwind out
# of one aborts the process. Each body runs under a guard whose `Drop` - which
# only an unwind runs - calls `makiri_unwinds_<site>`, a symbol nobody
# defines; built with `--cfg makiri_no_unwind` in the release profile, the
# optimizer removes that call exactly when it can prove the body never
# unwinds. So the symbols left undefined are the sites that can. The control
# site, `self_test`, can unwind by design: it must be the only one left, which
# also shows the check is live.
desc "Prove no extern \"C\" function or GC-run Drop can unwind (release build)"
task :no_unwind do
  macos = RbConfig::CONFIG["target_os"] =~ /darwin/
  dir = File.expand_path("ext/makiri/rust", __dir__)
  target = File.join(dir, "target", "no_unwind")
  # The check build is the crate alone, linked as cargo would; macOS's linker
  # refuses undefined symbols unless told otherwise, Linux's does not.
  link = macos ? %w[-C link-arg=-undefined -C link-arg=dynamic_lookup] : []
  env = { "RUSTFLAGS" => [ENV.fetch("RUSTFLAGS", nil), "--cfg makiri_no_unwind"].compact.join(" ") }
  Dir.chdir(dir) do
    sh env, "cargo", "rustc", "--release", "--lib", "--target-dir", target, "--", *link
  end
  lib = Dir[File.join(target, "release", "libmakiri.{dylib,so}")].first or abort "no_unwind: no library built"
  left = `nm -u #{lib.shellescape}`.lines.map(&:strip)
                                       .filter_map { |l| l[/(?:\A|\s)_?makiri_unwinds_(\w+)\z/, 1] }.uniq.sort
  abort "no_unwind: the control site was optimized away - the check is not live" unless left.include?("self_test")
  sites = left - ["self_test"]
  abort "no_unwind: these sites can unwind (see src/no_unwind.rs): #{sites.join(", ")}" unless sites.empty?
  puts "no_unwind: every site is unwind-free; the control is live"
end

# The vendored Lexbor, built alone. The extension no longer needs it - the
# arena DOM and the ported parsers are all Rust - so `rake compile` does not
# build it; what still does is everything that compares against Lexbor or
# builds its reference layers (`--all-features`, the `lexbor` feature, the
# differential fuzz targets), and those depend on this. Rebuilds only when the
# install is missing (script/build_lexbor.rb).
desc "Build the vendored Lexbor (the reference the ports are tested against)"
task :lexbor do
  sh FileUtils::RUBY, File.expand_path("script/build_lexbor.rb", __dir__)
end

# The lint gate, with CI's OWN arguments. It exists because a narrower spelling
# looks like it passes and is not: `cargo clippy --no-default-features --features
# lexbor` never compiles `bridge/` or `glue/` at all, so five `clippy.toml`
# violations there went unseen locally and turned CI red. `--all-features` is
# every configuration the crate has, which is why CI uses it - and why this task
# does, rather than leaving the arguments to be remembered.
desc "Clippy and rustfmt exactly as CI runs them (--all-features, warnings are errors)"
task lint: :lexbor do
  Dir.chdir("ext/makiri/rust") do
    sh "cargo", "clippy", "--all-features", "--tests", "--", "-D", "warnings"
    # The build that ships has no Lexbor: code only the reference layers use
    # is dead there, which `--all-features` cannot see - and so are test
    # helpers only a Lexbor test calls, which the html-dom test build sees.
    sh "cargo", "clippy", "--no-default-features", "--features", "ruby", "--", "-D", "warnings"
    sh "cargo", "clippy", "--no-default-features", "--features", "html-dom", "--tests", "--", "-D", "warnings"
    sh "cargo", "fmt", "--all", "--", "--check"
  end
  # The fuzz crate is a workspace of its own, which the crate's `--all` does
  # not reach; CI's "Fuzz crate checks" runs these two.
  Dir.chdir("ext/makiri/rust/fuzz") do
    sh "cargo", "fmt", "--", "--check"
    sh "cargo", "clippy", "--all-targets", "--", "-D", "warnings"
  end
end

# `rake dylint` - the crate's own lint (ext/makiri/rust/lints/makiri_lints),
# `ruby_value_on_heap`: a Ruby value kept where the GC's stack scan does not
# see it (a heap container, a static or thread-local, a wrapped object's data
# outside a traced field), the rule every Ruby extension keeps by hand, checked
# by type. The lint is a rustc plugin, so it builds on the nightly its
# rust-toolchain.toml pins, with cargo-dylint and dylint-link installed. The
# second run is the self-test: `lint_probe.rs`, compiled only under
# `--cfg makiri_lint_probe`, ends with `//~` each line the lint must report,
# and the task fails unless exactly those lines are reported - so a lint that
# has stopped seeing a case cannot pass by reporting nothing.
DYLINT = %w[cargo dylint --path lints/makiri_lints --no-deps --
            --no-default-features --features ruby].freeze

desc "Run the crate's own lint (makiri_lints; needs cargo-dylint and its nightly)"
task :dylint do
  require "open3"
  Dir.chdir("ext/makiri/rust") do
    # cargo keeps the crate's last check when only the lint library changed,
    # and dylint then answers from it - the old lints' verdict. A touch makes
    # every run check afresh.
    FileUtils.touch("src/lib.rs")
    sh(*DYLINT)
    probe = "src/lint_probe.rs"
    expected = File.readlines(probe).each_with_index
                   .select { |line, _| line.match?(%r{//~\s*\z}) }.map { |_, i| i + 1 }
    out, = Open3.capture2e({ "RUSTFLAGS" => "--cfg makiri_lint_probe" }, *DYLINT)
    reported = out.scan(%r{--> #{Regexp.escape(probe)}:(\d+):}).flatten.map(&:to_i).uniq.sort
    unless reported == expected
      puts out
      abort "makiri_lints' self-test: expected reports on lines #{expected.inspect} " \
            "of #{probe}, got #{reported.inspect}"
    end
    puts "makiri_lints: self-test reported all #{expected.size} cases"
  end
end

# `rake spec:valgrind` - run the spec suite under Valgrind memcheck via
# ruby_memcheck (Linux CI; see .github/workflows/valgrind.yml). The gem ships
# Ruby's own Valgrind suppression files (matched by Ruby version) and filters
# the report down to errors whose stack touches our extension, so we no longer
# have to fetch ruby.supp from ruby/ruby (that path was removed upstream).
#
# The job catches *use of uninitialised values* and *invalid reads/writes*
# (incl. intra-arena overflows), and also any "definitely lost" block whose
# stack touches our extension - the one leak gate with no frequency threshold
# (`rake leaks`, macOS, flags only stacks repeated >= 30x). It once meant to
# leave leaks out and could not: see `--leak-check` below. ruby_memcheck's
# defaults disable undef-value errors, so we override them.
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
      # Leaks ARE checked here, whatever this says: ruby_memcheck always runs
      # valgrind with --xml=yes, and in XML mode memcheck writes its leak
      # records even under --leak-check=no (a "definitely lost" in a spec's
      # child process failed run 37864917248 that way). So say what happens,
      # and keep ruby_memcheck's own leak kinds - these options REPLACE its
      # defaults rather than adding to them, and without the line below
      # valgrind's default (definite,possible) fails the job on a "possibly
      # lost" too. A spec that runs Ruby in a child must leave
      # RUBY_FREE_AT_EXIT set there (it is inherited): without it the child's
      # live strings at exit are reported as definitely lost through our
      # frames (PR #55).
      "--leak-check=full",
      "--show-leak-kinds=definite",
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

# The API surface manifest. Unlike the differential above, this one IS
# regenerable - it describes the current build, not a vanished one - so the
# discipline is different: re-record it in the same commit as the API change, so
# the diff reviews as an API change rather than as a regenerated blob.
# `spec/api_surface_spec.rb` is what compares it, so `rake spec` covers it.
namespace :api do
  desc "Re-record the API surface manifest (spec/api_surface.txt)"
  task record: :compile do
    sh FileUtils::RUBY, "-Ilib", "script/api_manifest.rb", "--record"
  end

  desc "Print the API surface manifest of this build"
  task show: :compile do
    sh FileUtils::RUBY, "-Ilib", "script/api_manifest.rb"
  end
end

# `rake clean` (from rake-compiler) removes the ext build dir under tmp/,
# including the generated Makefile. The next `rake compile` re-runs extconf, so
# a changed configure step (a new env flag, a different Ruby) is picked up. The
# Rust crate's sources do not need this - cargo finds a module from its `mod`
# declaration - but the crate is handed to cargo by that Makefile, so a stale
# one is still worth clearing. The vendored Lexbor build is deliberately NOT
# wiped here (it is slow to rebuild and rarely changes); use `rake clean:lexbor`
# for a from-scratch Lexbor build.
#
#   rake clean compile     # regenerate ext Makefile + recompile (fast)
#   rake clean:lexbor      # force a full Lexbor rebuild next `rake lexbor`

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

desc "Symbol gate: the built extension must export only Init_makiri (and ruby_abi_version)"
task symbols: :compile do
  lib = Dir["lib/makiri/makiri.{bundle,so}"].first or abort "no built extension"
  macos = RbConfig::CONFIG["target_os"] =~ /darwin/
  windows = RbConfig::CONFIG["target_os"] =~ /mingw|mswin/
  # macOS decorates C symbols with a leading underscore; Linux/Windows do not.
  u = macos ? "_" : ""

  # ONLY Init_makiri (and ruby_abi_version, which Ruby reads at require time)
  # may be EXPORTED. The hazard was Lexbor's: another Lexbor-based gem in the
  # same process binding to our differently-versioned copy and segfaulting -
  # see CLAUDE.md. No Lexbor is linked any more, and any name of ours in the
  # dynamic table is the same hazard for whatever binds it.
  #
  # (This gate also refused an UNDEFINED lxb_/lexbor_/mkr_ symbol, which macOS's
  # -undefined dynamic_lookup turns into a NULL call. Nothing so named is linked
  # or declared in the extension now, so that half checked nothing and went.)
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

  puts "symbols: exports limited to #{allowed.join(' + ')} (#{lib})"
end

desc "OOM-injection gate: rebuild with MAKIRI_ALLOC_INJECT=1 and sweep every core " \
     "allocation site, verifying each failure fails closed (clean raise or " \
     "baseline-identical result, never truncated output), then make every " \
     "allocating Ruby C call raise in turn, verifying no document is left borrowed"
task :oom do
  # The hook is compiled in only under MAKIRI_ALLOC_INJECT=1 (zero overhead in
  # a normal build), so this needs its own rebuild; see
  # script/check_alloc_failures.rb for the protocol and the property gated.
  sanitize = ENV["MAKIRI_SANITIZE"].to_s
  build_env = { "MAKIRI_ALLOC_INJECT" => "1" }
  build_env["MAKIRI_SANITIZE"] = sanitize unless sanitize.empty?
  sh(build_env, "#{FileUtils::RUBY} -S rake clean compile")

  run_env = {}
  unless sanitize.empty?
    assert_sanitized!("oom", sanitize)
    run_env = {
      "ASAN_OPTIONS" => ASAN_ENV_OPTIONS,
      "MAKIRI_SANITIZE" => sanitize,
    }.merge(asan_preload_env(sanitize))
  end
  sh(run_env, "#{FileUtils::RUBY} -Ilib script/check_alloc_failures.rb")
  # The same build carries the Ruby-raise hook: every allocating C call of
  # `bridge::raw` raises in turn, and no document may be left borrowed.
  sh(run_env, "#{FileUtils::RUBY} -Ilib script/check_raise_injection.rb")
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
  # Only the Ruby-free features: anything under glue needs rb-sys -> a live
  # Ruby, which Kani cannot build.
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
  #
  #   KANI_GROUPS=alloc,utf8 rake kani            # some groups (KANI_GROUPS below)
  #   KANI_JOBS=4 rake kani                       # harnesses in parallel
  argv = ["cargo", "kani", "--no-default-features"]
  harness = ENV["HARNESS"].to_s.strip
  groups = ENV["KANI_GROUPS"].to_s.split(",").map(&:strip).reject(&:empty?)
  unknown = groups - KANI_GROUPS.keys
  abort "rake kani: unknown KANI_GROUPS #{unknown.join(', ')} (known: #{KANI_GROUPS.keys.join(', ')})" if unknown.any?
  if !harness.empty?
    argv += ["--harness", harness]
  elsif groups.any?
    groups.flat_map { |g| KANI_GROUPS[g][:harnesses] }.each { |h| argv += ["--harness", h] }
  end
  # Kani runs harnesses in parallel only with its terse output, which still
  # names each harness and its verdict.
  jobs = ENV["KANI_JOBS"].to_s.strip
  argv += ["--jobs", jobs, "--output-format=terse"] unless jobs.empty?
  # A proof whose assertions no execution reaches - an `assume` too strong, a
  # bound too small - still reports "0 failed". So every harness states, with
  # `kani::cover!`, that its claim is reached, and Kani (which only reports a
  # cover it cannot satisfy) is held to it here: each harness must report
  # covers, all of them satisfied.
  puts argv.join(" ")
  out = +""
  ok = Dir.chdir("ext/makiri/rust") do
    IO.popen(argv, err: %i[child out]) do |io|
      io.each_line { |line| print line; out << line }
    end
    $?.success?
  end
  abort "rake kani: cargo kani failed" unless ok
  harnesses = out.scan(/Checking harness (\S+?)\.\.\./).flatten.uniq
  covers = out.scan(/\*\* (\d+) of (\d+) cover properties satisfied/)
  unsatisfied = covers.count { |got, of| got != of }
  if covers.length != harnesses.length || unsatisfied.positive?
    abort "rake kani: #{harnesses.length} harnesses, #{covers.length} reported covers, " \
          "#{unsatisfied} with a cover unsatisfied - every harness needs a kani::cover! " \
          "its claim reaches, and each must be satisfiable (see the output above)"
  end
  puts "rake kani: every harness's covers satisfied (#{harnesses.length} harnesses)"
end

# The Kani harnesses in groups, each with the code its proofs depend on, so CI
# runs a group only when that code changed (verify.yml; a weekly run takes all
# of them, to catch a dependency missed here). The paths are the code
# each group's proofs run - read off the harnesses' `use`s, what the proven
# functions call, and the modules' `cfg(kani)`: `cbuf` uses only `falloc`,
# which uses nothing, while `xml::verify` reaches `cutf8` through
# `xml::chars`. `KANI_SHARED` is what every group depends on - the
# bounds in lib.rs, the build, this file and the workflow. `kani:groups`
# fails when a file holding a proof is in no group, so a new harness cannot
# go unrun.
RUST_SRC = "ext/makiri/rust/src"
KANI_GROUPS = {
  # The allocator and the byte buffer: the slow group (~6 of CI's ~10 min).
  "alloc" => {
    harnesses: %w[cbuf::verify:: falloc::verify::],
    paths: %W[#{RUST_SRC}/cbuf.rs #{RUST_SRC}/cbuf/ #{RUST_SRC}/falloc/]
  },
  # `RawVec`, a NodeSet's node array: its growth over `falloc::grow_capacity`.
  "raw_vec" => {
    harnesses: %w[raw_vec::verify::],
    paths: %W[#{RUST_SRC}/raw_vec/ #{RUST_SRC}/falloc/ #{RUST_SRC}/no_unwind.rs]
  },
  "utf8" => {
    harnesses: %w[cutf8::verify::],
    paths: %W[#{RUST_SRC}/cutf8.rs #{RUST_SRC}/cutf8/]
  },
  # `cutf8` too: `xml::chars::decode1` IS `cutf8::decode1`, and
  # `validate_chars` decodes with it, so a change there can change these
  # proofs' answer - which the utf8 group's own proofs of `decode1` do not
  # stand in for. Not `falloc`, which `xml::chars` uses only outside what
  # these prove (`normalize_newlines`).
  "xml" => {
    harnesses: %w[xml::verify::],
    paths: %W[#{RUST_SRC}/xml/chars/ #{RUST_SRC}/xml/verify.rs #{RUST_SRC}/xml/mod.rs
              #{RUST_SRC}/cutf8.rs #{RUST_SRC}/cutf8/]
  },
  "xpath" => {
    harnesses: %w[xpath::verify::],
    paths: %W[#{RUST_SRC}/xpath/number.rs #{RUST_SRC}/xpath/verify.rs #{RUST_SRC}/xpath/mod.rs]
  }
}.freeze
KANI_SHARED = %W[
  #{RUST_SRC}/lib.rs ext/makiri/rust/build.rs ext/makiri/rust/Cargo.toml
  ext/makiri/rust/Cargo.lock Rakefile .github/workflows/verify.yml
].freeze

namespace :kani do
  # Print the groups the changed files (one per line on stdin) touch, comma
  # separated - all of them when a shared file changed, none when no proof's
  # code did. First checks that every file holding a `#[kani::proof]` is in a
  # group.
  desc "Kani groups touched by the files on stdin (and check every proof is grouped)"
  task :groups do
    covered = ->(file, paths) { paths.any? { |p| p.end_with?("/") ? file.start_with?(p) : file == p } }
    all_paths = KANI_GROUPS.values.flat_map { |g| g[:paths] }
    ungrouped = Dir.glob("#{RUST_SRC}/**/*.rs").select { |f| File.read(f).include?("#[kani::proof]") }
                   .reject { |f| covered.call(f, all_paths) }
    abort "kani:groups: proofs in no group: #{ungrouped.join(', ')}" if ungrouped.any?

    changed = $stdin.read.split("\n").map(&:strip).reject(&:empty?)
    hit = if changed.any? { |f| KANI_SHARED.include?(f) }
            KANI_GROUPS.keys
          else
            KANI_GROUPS.select { |_, g| changed.any? { |f| covered.call(f, g[:paths]) } }.keys
          end
    puts hit.join(",")
  end
end

namespace :rust do
  # The Ruby- and Lexbor-free core; the HTML DOM the extension ships with,
  # still Lexbor-free; then the same with the `lexbor` layers in, which holds
  # the ports to Lexbor (the reference tests) and needs the vendored Lexbor
  # `rake lexbor` builds.
  desc "Run the Rust tests (the engine core, the shipped DOM, then against Lexbor)"
  task test: :lexbor do
    Dir.chdir("ext/makiri/rust") do
      sh "cargo test --no-default-features"
      sh "cargo test --no-default-features --features html-dom"
      sh "cargo test --no-default-features --features lexbor"
      # The fuzz crate is a separate one, so `cargo test` above never sees it;
      # without this check an engine API change can silently break the nightly
      # harnesses (they are not built by `rake compile` or `rake spec`).
      sh "cargo check --manifest-path fuzz/Cargo.toml"
    end
  end
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

desc "CSS engines on one document as HTML and XML (bench/bench_css.rb; CSS_ARGS=--json PATH / --compare PATH)"
task "bench:css" => :compile do
  sh "#{FileUtils::RUBY} -Ilib bench/bench_css.rb #{ENV['CSS_ARGS']}"
end

namespace :conformance do
  desc "WHATWG HTML parsing conformance: the WPT tree-construction data through Makiri"
  task html5: :compile do
    sh "#{FileUtils::RUBY} -Ilib spec/conformance/html5lib_runner.rb #{ENV['H5_ARGS']}"
  end

  desc "HTML parser conformance in the crate: WPT tree construction through each parser"
  task parser: :lexbor do
    sh "#{FileUtils::RUBY} spec/conformance/parser_data.rb"
    Dir.chdir("ext/makiri/rust") do
      sh "cargo test --no-default-features --features lexbor -- --include-ignored wpt html5lib entities_are_the_whatwg res_gen"
    end
  end

  # The ported tree builder against Lexbor over 100,000 real homepages
  # (web100k, ~3.1 GB downloaded batch by batch). A manual check, not in CI;
  # BATCHES=1,2 narrows it, KEEP=1 keeps the decompressed pages.
  desc "HTML parser port vs Lexbor over the web100k corpus (BATCHES=1,2; needs zstd)"
  task :web100k do
    sh "#{FileUtils::RUBY} spec/conformance/web100k.rb"
  end

  # Evidence, not a gate: docs/rewrite_parser.ja.md makes the pinned HTML
  # Standard the authority, and browsers can all predate a spec change.
  desc "HTML parser vs Chromium/Firefox/WebKit (Playwright; needs Node.js)"
  task browsers: :compile do
    dir = "spec/conformance/browser"
    unless File.directory?("#{dir}/node_modules/playwright-core")
      sh "npm", "install", "--no-audit", "--no-fund", "--prefix", dir
    end
    sh "#{FileUtils::RUBY} -Ilib spec/conformance/browser_diff.rb #{ENV['BROWSER_ARGS']}"
  end

  desc "CSS selectors vs Chromium/Firefox/WebKit, HTML and XML (Playwright; needs Node.js)"
  task css_browsers: :compile do
    dir = "spec/conformance/browser"
    unless File.directory?("#{dir}/node_modules/playwright-core")
      sh "npm", "install", "--no-audit", "--no-fund", "--prefix", dir
    end
    sh "#{FileUtils::RUBY} -Ilib spec/conformance/css_browser_diff.rb #{ENV['BROWSER_ARGS']}"
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
  #   ISOLATED=0    same as FAST=1; ISOLATED=1 (like the default) keeps isolation.
  #   SKIP_BUILD=1  reuse the current build instead of rebuilding (refuses to run
  #                 if it is not a sanitizer build, so you never fuzz a plain ext).
  #   FUZZ_TIME=N   seconds per surface (default 90).
  #   FUZZ_ARGS=... run a single custom invocation instead of the five surfaces.
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
      truthy = ->(name) { %w[1 true yes].include?(ENV[name].to_s.downcase) }
      explicit_isolated = ENV.key?("ISOLATED") && !ENV["ISOLATED"].to_s.strip.empty?
      isolated = explicit_isolated ? truthy.call("ISOLATED") : !truthy.call("FAST")
      iso = isolated ? "--isolated" : "--in-process"
      secs = ENV["FUZZ_TIME"] || "90"
      # Cover every surface under the sanitizer: the query engine (XPath over the
      # default fixtures), the HTML CSS selector path, the XML parser (hostile
      # documents), the XML mutation surface, and CSS over an XML document.
      ["", "--target css", "--target xml", "--target mutate", "--target xmlcss"].each do |surface|
        sh(env, "#{FileUtils::RUBY} -Ilib spec/fuzz/run.rb #{surface} #{iso} --time #{secs}".squeeze(" ").strip)
      end
    end
  end

  # Coverage-guided libFuzzer harnesses for the Ruby-free surfaces (the XML
  # reader and the HTML parse, XPath over both engine instances, and the CSS
  # lowering). They are
  # standalone binaries, so they run without the Ruby interpreter, and they
  # complement the Ruby-based robustness fuzzer by providing coverage feedback
  # and 2-3 orders of magnitude faster execution for the engine core.
  #
  # Nothing here depends on the built extension: cargo-fuzz builds the crate
  # itself, with its own instrumentation. It does need the vendored Lexbor
  # library and headers (the crate's build.rs links the archive and generates
  # the layout from the headers), which `rake compile` produces - hence the
  # dependency, which is about Lexbor rather than about the bundle.
  FUZZ_TARGETS = %w[xml xpath xml_xpath css html html_xpath html_css html_css_diff html_diff selector_diff stylesheet_diff].freeze
  # The targets whose input starts with a CSS selector take its dictionary.
  FUZZ_CSS_DICT = %w[html_css html_css_diff].freeze

  # The local mode: TARGETS=css,html_xpath narrows the run to what you touched,
  # and the time is FUZZ_TIME seconds per target (default 60) or FUZZ_BUDGET
  # seconds for the whole run, split evenly across the targets.
  def libfuzzer_targets
    wanted = ENV["TARGETS"].to_s.split(",").map(&:strip).reject(&:empty?)
    return FUZZ_TARGETS if wanted.empty?

    unknown = wanted - FUZZ_TARGETS
    unknown.empty? or
      abort "fuzz:libfuzzer: unknown TARGETS #{unknown.join(', ')} " \
            "(have: #{FUZZ_TARGETS.join(', ')})"
    wanted
  end

  def libfuzzer_seconds(count)
    return [ENV["FUZZ_BUDGET"].to_i / count, 1].max if ENV["FUZZ_BUDGET"]

    (ENV["FUZZ_TIME"] || "60").to_i
  end

  # The environment and sanitizer flag for a cargo-fuzz invocation. PLATFORM-
  # SPLIT, like the ASan preload: an ASan-instrumented harness never reaches
  # main on macOS - its runtime re-enters its own init through malloc during
  # dyld's initialisers and spins forever, the same deadlock recorded for the
  # preload in CLAUDE.md. So macOS runs uninstrumented (`-s none`): coverage
  # guidance and the harnesses' own asserts, with memory safety left to
  # `fuzz:sanitize` and the Linux CI run. Without ASan's runtime nothing in the
  # crate's cdylib defines the `__sanitizer_cov_*` hooks it is instrumented
  # with (libFuzzer does, but only in the harness binary), so that link needs
  # `-undefined dynamic_lookup`; the harness resolves them at load. Linux keeps
  # ASan. RUSTUP_TOOLCHAIN defaults to nightly, which cargo-fuzz requires.
  def libfuzzer_invocation
    env = { "RUSTUP_TOOLCHAIN" => ENV["RUSTUP_TOOLCHAIN"] || "nightly" }
    return [env, []] unless RbConfig::CONFIG["target_os"] =~ /darwin/

    flags = [ENV["RUSTFLAGS"], "-Clink-arg=-Wl,-undefined,dynamic_lookup"].compact.join(" ")
    [env.merge("RUSTFLAGS" => flags), %w[--sanitizer none]]
  end

  desc "Build the cargo-fuzz harnesses (requires cargo-fuzz and a nightly toolchain)"
  task :libfuzzer_build => %i[compile lexbor] do
    cargo_fuzz_available? or
      abort "fuzz:libfuzzer_build: cargo-fuzz is not installed " \
            "(`cargo install cargo-fuzz`; it needs a nightly toolchain)."
    env, sanitizer = libfuzzer_invocation
    Dir.chdir("ext/makiri/rust/fuzz") do
      libfuzzer_targets.each { |target| sh env, "cargo", "fuzz", "build", *sanitizer, target }
    end
  end

  desc "Run the cargo-fuzz harnesses (TARGETS=a,b; FUZZ_TIME per target or FUZZ_BUDGET total)"
  task :libfuzzer => :libfuzzer_build do
    targets = libfuzzer_targets
    time = libfuzzer_seconds(targets.size)
    env, sanitizer = libfuzzer_invocation
    Dir.chdir("ext/makiri/rust/fuzz") do
      targets.each do |target|
        dict = FUZZ_CSS_DICT.include?(target) ? ["-dict=css.dict"] : []
        sh env, "cargo", "fuzz", "run", *sanitizer, target, "--",
           "-max_total_time=#{time}", "-max_len=4096", *dict
      end
    end
  end
end

desc "Show code statistics"
task :stats do
  sh "tokei lib ext spec script --exclude tmp --exclude vendor --exclude docs"
end
