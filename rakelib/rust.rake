# frozen_string_literal: true

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
  ext/makiri/rust/Cargo.lock Rakefile rakelib/rust.rake .github/workflows/verify.yml
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
      # The binding layer, a workspace member of its own (its Ruby-free tests:
      # the message buffer, and `caught`'s panic latch and payload reading).
      sh "cargo test -p nupur"
      # The fuzz crate is a separate one, so `cargo test` above never sees it;
      # without this check an engine API change can silently break the nightly
      # harnesses (they are not built by `rake compile` or `rake spec`).
      sh "cargo check --manifest-path fuzz/Cargo.toml"
    end
  end
end
