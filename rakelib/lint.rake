# frozen_string_literal: true

# The vendored Lexbor, built alone. The extension no longer needs it - the
# arena DOM and the ported parsers are all Rust - so `rake compile` does not
# build it; what still does is everything that compares against Lexbor or
# builds its reference layers (`--all-features`, the `lexbor` feature, the
# differential fuzz targets), and those depend on this. Rebuilds only when the
# install is missing (script/build_lexbor.rb).
desc "Build the vendored Lexbor (the reference the ports are tested against)"
task :lexbor do
  sh FileUtils::RUBY, File.expand_path("script/build_lexbor.rb", ROOT)
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
    # The binding layer is a workspace member of its own, which a command run
    # in the root does not reach.
    sh "cargo", "clippy", "-p", "nupur", "--all-features", "--tests", "--", "-D", "warnings"
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
# The binding layer, a workspace member of its own: the same lints, which
# read its items as `nupur::...` (`paths::path_of`).
# With `alloc-inject` on, so the raise sweep's hook (`set_raise_injector`,
# the armed path of `injected_raise`) is linted too: nupur has no code that
# only a build without it compiles.
DYLINT_NUPUR = %w[cargo dylint --path lints/makiri_lints --no-deps -- -p nupur
                  --features alloc-inject].freeze

desc "Run the crate's own lint (makiri_lints; needs cargo-dylint and its nightly)"
task :dylint do
  require "open3"
  Dir.chdir("ext/makiri/rust") do
    # cargo keeps the crate's last check when only the lint library changed,
    # and dylint then answers from it - the old lints' verdict. A touch makes
    # every run check afresh.
    FileUtils.touch("src/lib.rs")
    FileUtils.touch("nupur/src/lib.rs")
    sh(*DYLINT)
    sh(*DYLINT_NUPUR)
    # The self-test, once per crate: the extension's probe, and the binding
    # layer's, which holds the cases only code naming rb-sys can write.
    probes = [[DYLINT, "src/lint_probe.rs", "makiri_lint_probe"],
              [DYLINT_NUPUR, "nupur/src/lint_probe.rs", "nupur_lint_probe"]]
    total = probes.sum do |cmd, probe, cfg|
      expected = File.readlines(probe).each_with_index
                     .select { |line, _| line.match?(%r{//~\s*\z}) }.map { |_, i| i + 1 }
      out, = Open3.capture2e({ "RUSTFLAGS" => "--cfg #{cfg}" }, *cmd)
      reported = out.scan(%r{--> #{Regexp.escape(probe)}:(\d+):}).flatten.map(&:to_i).uniq.sort
      unless reported == expected
        puts out
        abort "makiri_lints' self-test: expected reports on lines #{expected.inspect} " \
              "of #{probe}, got #{reported.inspect}"
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
