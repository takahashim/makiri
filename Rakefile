# frozen_string_literal: true

require "bundler/gem_tasks"
require "rspec/core/rake_task"
require "rake/extensiontask"
require "shellwords"

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

# The compiled extension, and whether it carries sanitizer instrumentation, so
# `fuzz:sanitize SKIP_BUILD=1` can refuse to run a plain (non-ASan) build.
def ext_bundle_path
  Dir["lib/makiri/makiri.{bundle,so}"].first
end

def ext_sanitized?
  bundle = ext_bundle_path or return false
  !(`nm "#{bundle}" 2>/dev/null` =~ /asan|ubsan/i).nil?
end

desc "Build the extension with sanitizers (MAKIRI_SANITIZE, default " \
     "address,undefined) and run the spec suite under them"
task :sanitize do
  sanitize = ENV["MAKIRI_SANITIZE"] || "address,undefined"
  sh({ "MAKIRI_SANITIZE" => sanitize }, "#{FileUtils::RUBY} -S rake clean compile")

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
      ext_sanitized? or
        abort "fuzz:sanitize: SKIP_BUILD set but lib/makiri is not a sanitizer build; " \
              "drop SKIP_BUILD to rebuild with MAKIRI_SANITIZE"
      puts "fuzz:sanitize: reusing the existing sanitizer build (SKIP_BUILD)"
    else
      sh({ "MAKIRI_SANITIZE" => sanitize }, "#{FileUtils::RUBY} -S rake clean compile")
    end

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
end
