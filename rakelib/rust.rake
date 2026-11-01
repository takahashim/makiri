# frozen_string_literal: true

# `rake verify` (CBMC over the C carve-out) is gone with the C it proved, and
# `rake kani`, its successor over the Rust, is gone too: what its harnesses
# proved bounded, Verus proves at every length (ext/makiri/rust/verus/,
# `rake verus`), and the one claim Verus cannot state - that the strict UTF-8
# decoder agrees with std's `from_utf8`, which it has no contract for - is a
# test that runs every input (`rust:exhaustive`).

namespace :rust do
  # The bare engine (no HTML DOM); the engine as the extension builds it,
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
      # The crate under it: the integer conversions at both widths of a
      # `long`, and the compile-fail doctests that keep its tokens (`Value`,
      # `Ruby`, `Protected`) where they are made.
      sh "cargo test -p nupur_sys"
      sh "cargo test --manifest-path reference/Cargo.toml"
      # The fuzz crate is a separate one, so `cargo test` above never sees it;
      # without this check an engine API change can silently break the nightly
      # harnesses (they are not built by `rake compile` or `rake spec`).
      sh "cargo check --manifest-path fuzz/Cargo.toml"
    end
  end

  # The tests that run every input of their domain: too many for a debug
  # build, a few seconds in release on a few cores. `#[ignore]`d so that
  # `rust:test` leaves them out; each lives in a module named `exhaustive`.
  # CI: verify.yml.
  desc "Run the exhaustive Rust tests in release (decode1 against from_utf8 on every input)"
  task :exhaustive do
    Dir.chdir("ext/makiri/rust") do
      sh "cargo test --release -p makiri_engine --no-default-features --lib -- --ignored exhaustive::"
    end
  end

  # The compile-fail doctests, on nightly. Each names the error it expects
  # (`compile_fail,E0451`), and only nightly's rustdoc checks that code: a
  # stable run passes a snippet that fails for any reason at all, a typo
  # included. The workspace's crates that have them; the Lexbor reference's
  # one is left to its own run.
  desc "Check the compile-fail doctests' error codes (nightly)"
  task :compile_fail do
    Dir.chdir("ext/makiri/rust") do
      sh "cargo +nightly test --doc -p makiri_engine -p falloc -p nupur_sys"
    end
  end
end
