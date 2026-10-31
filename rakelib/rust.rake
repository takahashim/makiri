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
desc "Kani proofs over the Ruby-free core - the UTF-8 decoder against std's " \
     "validator, XML character classes (needs cargo-kani; successor to the C-era CBMC harnesses)"
task :kani do
  # The engine alone, with no feature: the extension needs rb-sys -> a live
  # Ruby, which Kani cannot build, and the engine knows no Ruby.
  #
  #   rake kani                                   # every harness
  #   rake kani HARNESS=predicate_inclusions      # one
  #   KANI_GROUPS=utf8,xml rake kani              # some groups (KANI_GROUPS below)
  #   KANI_JOBS=4 rake kani                       # harnesses in parallel
  argv = ["cargo", "kani", "-p", "makiri_engine", "--no-default-features"]
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
# functions call, and the modules' `cfg(kani)`. `KANI_SHARED` is what every group depends on - the
# bounds in lib.rs, the build, this file and the workflow. `kani:groups`
# fails when a file holding a proof is in no group, so a new harness cannot
# go unrun.
RUST_SRC = "ext/makiri/rust/engine/src"
KANI_GROUPS = {
  # (An "alloc" group proved falloc's grow_capacity and ceiling_buf's copy_small,
  # bounded, in ~6 of CI's ~10 minutes. verus/ceiling_buf.rs proves both at every
  # size, and ceiling_buf's tests run the rest, so it went.)
  "utf8" => {
    harnesses: %w[utf8_decode::verify::],
    paths: %W[#{RUST_SRC}/utf8_decode.rs #{RUST_SRC}/utf8_decode/]
  },
  # The character classes alone: no decode is proved here any more. Not
  # `falloc`, which `xml::chars` uses only outside what this proves
  # (`normalize_newlines`).
  "xml" => {
    harnesses: %w[xml::verify::],
    paths: %W[#{RUST_SRC}/xml/chars/ #{RUST_SRC}/xml/verify.rs #{RUST_SRC}/xml/mod.rs]
  }
  # (An "xpath" group proved `xpath::number::extent` up to 6 bytes, and the
  # utf8 and xml groups held the decoder's contract, the decode chain and
  # `validate_chars` up to 8: verus/xpath_number.rs and verus/utf8.rs prove
  # those at every length.)
}.freeze
KANI_SHARED = %W[
  #{RUST_SRC}/lib.rs ext/makiri/rust/engine/Cargo.toml ext/makiri/rust/Cargo.toml
  ext/makiri/rust/Cargo.lock ext/makiri/rust/falloc/src/lib.rs
  Rakefile rakelib/rust.rake .github/workflows/verify.yml
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
  # The bare engine (what Kani proves); the engine as the extension builds it,
  # with the HTML DOM; falloc, its injection hook alone in a binary of its own
  # (the counter is process-global); the binding layer; and then the Lexbor
  # reference, which holds the ports to Lexbor and needs the vendored Lexbor
  # `rake lexbor` builds. The extension itself has no Ruby-free test to run.
  desc "Run the Rust tests (the engine bare and as shipped, falloc, nupur, then against Lexbor)"
  task test: :lexbor do
    Dir.chdir("ext/makiri/rust") do
      sh "cargo test -p makiri_engine --no-default-features"
      sh "cargo test -p makiri_engine"
      sh "cargo test -p falloc"
      sh "cargo test -p falloc --features alloc-inject --test alloc_inject_map_insert"
      # The binding layer (its Ruby-free tests: the message buffer, and
      # `caught`'s panic latch and payload reading).
      sh "cargo test -p nupur"
      sh "cargo test --manifest-path reference/Cargo.toml"
      # The fuzz crate is a separate one, so `cargo test` above never sees it;
      # without this check an engine API change can silently break the nightly
      # harnesses (they are not built by `rake compile` or `rake spec`).
      sh "cargo check --manifest-path fuzz/Cargo.toml"
    end
  end
end
