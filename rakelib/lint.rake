# frozen_string_literal: true

# The vendored Lexbor, built alone. The extension does not need it - the
# arena DOM and the ported parsers are all Rust - so `rake compile` does not
# build it; what does is everything that compares against Lexbor (the
# reference crate, `reference/`, and the differential fuzz targets), and
# those depend on this. Rebuilds only when the
# install is missing (script/build_lexbor.rb).
desc "Build the vendored Lexbor (the reference the ports are tested against)"
task :lexbor do
  sh FileUtils::RUBY, File.expand_path("script/build_lexbor.rb", ROOT)
end

# The lint gate, with CI's OWN arguments (ci.yml's Clippy, Rust formatting and
# Fuzz crate checks steps, and the lints crate's fmt in its makiri_lints job). It exists because a narrower spelling
# looks like it passes and is not: a command run in the workspace root reaches
# the root crate alone, so five `clippy.toml` violations in a crate it did not
# reach went unseen locally and turned CI red. So every crate is named here,
# with every configuration it has, rather than left to be remembered.
desc "Clippy and rustfmt exactly as CI runs them (every crate, warnings are errors)"
task lint: :lexbor do
  Dir.chdir("ext/makiri/rust") do
    # The extension, the engine, falloc and nupur, each with every feature.
    sh "cargo", "clippy", "--workspace", "--all-features", "--tests", "--", "-D", "warnings"
    # The bare engine - no HTML DOM - where code only the DOM
    # uses is dead; and as the extension builds it, where test helpers only
    # the Lexbor reference calls are (`--all-features` sees neither).
    sh "cargo", "clippy", "-p", "makiri_engine", "--no-default-features", "--tests", "--", "-D", "warnings"
    sh "cargo", "clippy", "-p", "makiri_engine", "--tests", "--", "-D", "warnings"
    sh "cargo", "fmt", "--all", "--", "--check"
    # nupur's documentation is what an extension on the layer reads first:
    # a link that resolves nowhere, or to an item it cannot see, fails.
    sh({ "RUSTDOCFLAGS" => "-D warnings" }, "cargo", "doc", "-p", "nupur", "--no-deps", "--all-features")
  end
  # The Lexbor reference is a workspace of its own, which `--workspace` does
  # not reach.
  Dir.chdir("ext/makiri/rust/reference") do
    sh "cargo", "clippy", "--all-features", "--tests", "--", "-D", "warnings"
    sh "cargo", "fmt", "--", "--check"
  end
  # The lints are a crate of their own (rake dylint builds them); their
  # formatting is checked here, with everything else's.
  Dir.chdir("ext/makiri/rust/lints/makiri_lints") do
    sh "cargo", "fmt", "--", "--check"
  end
  # nupur's test extension is a crate of its own too (rake nupur:spec).
  Dir.chdir("ext/makiri/rust/nupur/test/ext") do
    sh "cargo", "fmt", "--", "--check"
    sh "cargo", "clippy", "--", "-D", "warnings"
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
# `--cfg makiri_lint_probe`, ends each line a lint must report with `//~` and
# the lints' names, and the task fails unless exactly those (line, lint)
# pairs are reported - so a lint that has stopped seeing a case cannot pass
# by reporting nothing, nor behind another lint reporting the same line.
DYLINT = %w[cargo dylint --path lints/makiri_lints --no-deps -- -p makiri_rs].freeze
# The engine: what it may allocate (`infallible_allocation`) and call
# (`runs_callers_ruby` has nothing to find there - it cannot name Ruby - but
# the shipped crate is the extension and the engine together).
DYLINT_ENGINE = %w[cargo dylint --path lints/makiri_lints --no-deps -- -p makiri_engine].freeze
# The binding layer, a workspace member of its own: the same lints, which
# read its items as `nupur::...` (`paths::path_of`).
# With `alloc-inject` on, so the raise sweep's hook (`set_raise_injector`,
# the armed path of `injected_raise`) is linted too: nupur has no code that
# only a build without it compiles.
DYLINT_NUPUR = %w[cargo dylint --path lints/makiri_lints --no-deps -- -p nupur
                  --features alloc-inject].freeze
# The crate under it that alone depends on rb-sys (`nupur/sys/`): the
# protect thunk, `$!` and the `no_unwind` guards, held to the layer's rules.
# With `alloc-inject` on, as nupur's run: the raise hook's armed path
# (`protect::injected_raise`, `set_raise_injector`) lives here now, and
# nupur's run, being `--no-deps`, does not lint it.
DYLINT_NUPUR_SYS = %w[cargo dylint --path lints/makiri_lints --no-deps -- -p nupur_sys
                      --features alloc-inject].freeze

desc "Run the crate's own lint (makiri_lints; needs cargo-dylint and its nightly)"
task :dylint do
  require "json"
  require "open3"
  Dir.chdir("ext/makiri/rust") do
    # cargo keeps the crate's last check when only the lint library changed,
    # and dylint then answers from it - the old lints' verdict. A touch makes
    # every run check afresh.
    #
    # Each crate is touched just before its own run: a run checks the crates
    # it depends on without the lints (`--no-deps`), and cargo then counts
    # that check as fresh, so the engine's and `nupur_sys`'s own runs, after
    # the extension's, linted nothing.
    [["src/lib.rs", DYLINT], ["engine/src/lib.rs", DYLINT_ENGINE],
     ["nupur/src/lib.rs", DYLINT_NUPUR], ["nupur/sys/src/lib.rs", DYLINT_NUPUR_SYS]].each do |root, cmd|
      FileUtils.touch(root)
      sh(*cmd)
    end
    # The self-test, once per crate: the extension's probe, and the binding
    # layer's, which holds the cases only code naming rb-sys can write. Each
    # `//~` names the lints that must report its line, and the reports must
    # be exactly those pairs: matched by line alone, a line that two lints
    # report kept passing with one of them dead (`ruby_in_gvl_released_body`
    # did, once `without_gvl` took the token).
    probes = [[DYLINT, "src/lint_probe.rs", "makiri_lint_probe"],
              [DYLINT_NUPUR, "nupur/src/lint_probe.rs", "nupur_lint_probe"],
              [DYLINT_NUPUR_SYS, "nupur/sys/src/lint_probe.rs", "nupur_sys_lint_probe"]]
    total = probes.sum do |cmd, probe, cfg|
      expected = File.readlines(probe).each_with_index.flat_map do |line, i|
        names = line[%r{//~((?: [a-z_]+)*)\s*\z}, 1] or next []
        abort "makiri_lints' self-test: #{probe}:#{i + 1}'s //~ names no lint" if names.strip.empty?
        names.split.map { |lint| [i + 1, lint] }
      end.sort
      out, = Open3.capture2e({ "RUSTFLAGS" => "--cfg #{cfg}" }, *cmd, "--message-format=json")
      messages = out.lines.filter_map do |l|
        m = JSON.parse(l) if l.start_with?("{")
        m["message"] if m && m["reason"] == "compiler-message"
      end
      reported = messages.filter_map do |m|
        at = m["spans"].find { |sp| sp["is_primary"] }
        [at["line_start"], m.dig("code", "code")] if at && at["file_name"] == probe && m["code"]
      end.uniq.sort
      unless reported == expected
        messages.each { |m| puts m["rendered"] }
        abort "makiri_lints' self-test on #{probe}: missing #{(expected - reported).inspect}, " \
              "unexpected #{(reported - expected).inspect} ([line, lint])"
      end
      expected.size
    end
    puts "makiri_lints: self-test reported all #{total} cases"
  end
  # The table the raising lints read (`NON_RAISING`) against Ruby's own
  # headers, as rb-sys's bindings carry them: an entry documented to raise,
  # or that never returns, is wrong (script/check_non_raising.rb).
  sh FileUtils::RUBY, "script/check_non_raising.rb"
end
