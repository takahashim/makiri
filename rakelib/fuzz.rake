# frozen_string_literal: true

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
  # matcher over both DOMs). They are
  # standalone binaries, so they run without the Ruby interpreter, and they
  # complement the Ruby-based robustness fuzzer by providing coverage feedback
  # and 2-3 orders of magnitude faster execution for the engine core.
  #
  # Nothing here depends on the built extension: cargo-fuzz builds the crate
  # itself, with its own instrumentation. It does need the vendored Lexbor
  # library and headers (the reference crate's build.rs links the archive and
  # generates the layout from the headers), which `rake lexbor` produces -
  # hence the one dependency.
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
  # preload in docs/internals/build_and_sanitizers.md. So macOS runs
  # uninstrumented (`-s none`): coverage guidance and the harnesses' own
  # asserts, with memory safety left to
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
  task :libfuzzer_build => :lexbor do
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
