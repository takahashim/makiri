# frozen_string_literal: true

# Miri over the Ruby-free code that holds `unsafe` - the `falloc` crate
# (`try_box`) and `nupur::utf8_copy`, which its test build includes: their
# tests on Miri's interpreter, which reports undefined behaviour a run on
# hardware passes over (CI: verify.yml). The engine forbids `unsafe`, so Miri
# would only be running std there. Needs the nightly toolchain with the
# `miri` component. MIRI_FILTERS narrows the tests.
MIRI_FILTERS = [].freeze
desc "Run the Ruby-free unsafe modules' tests under Miri (nightly)"
task :miri do
  filters = ENV["MIRI_FILTERS"] ? ENV["MIRI_FILTERS"].split(",") : MIRI_FILTERS
  env = { "MIRIFLAGS" => ENV.fetch("MIRIFLAGS", "-Zmiri-strict-provenance") }
  Dir.chdir("ext/makiri/rust") do
    sh env, "cargo", "+nightly", "miri", "test", "-p", "falloc", "--lib", "--", *filters
  end
end

# The places that must not unwind (`engine/src/no_unwind.rs`): every `extern "C"`
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
  dir = File.expand_path("ext/makiri/rust", ROOT)
  target = File.join(dir, "target", "no_unwind")
  # The check build is the crate alone, linked as cargo would; macOS's linker
  # refuses undefined symbols unless told otherwise, Linux's does not.
  link = macos ? %w[-C link-arg=-undefined -C link-arg=dynamic_lookup] : []
  # Each crate's own cfg: the extension's sites, and nupur's.
  env = { "RUSTFLAGS" => [ENV.fetch("RUSTFLAGS", nil), "--cfg makiri_no_unwind --cfg nupur_no_unwind"].compact.join(" ") }
  Dir.chdir(dir) do
    # `cdylib` alone, as the extension is built (extconf.rb): with the `rlib`
    # type in, rustc runs no LTO, and the proof would be of another binary.
    sh env, "cargo", "rustc", "--release", "--lib", "--crate-type", "cdylib", "--target-dir", target,
       "--", *link
  end
  lib = Dir[File.join(target, "release", "libmakiri.{dylib,so}")].first or abort "no_unwind: no library built"
  # Two mechanisms, one per layer: Makiri's (`engine/src/no_unwind.rs`, the
  # engine's `Drop`s) and nupur's (`nupur/src/no_unwind.rs`, the GC
  # callbacks, the protect thunk, the trampolines), each with its control.
  left = `nm -u #{lib.shellescape}`.lines.map(&:strip)
                                       .filter_map { |l| l[/(?:\A|\s)_?((?:makiri|nupur)_unwinds_\w+)\z/, 1] }.uniq.sort
  controls = %w[makiri_unwinds_self_test nupur_unwinds_self_test]
  dead = controls - left
  abort "no_unwind: a control site was optimized away - the check is not live: #{dead.join(", ")}" unless dead.empty?
  sites = left - controls
  abort "no_unwind: these sites can unwind (see engine/src/no_unwind.rs, nupur/src/no_unwind.rs): #{sites.join(", ")}" unless sites.empty?
  puts "no_unwind: every site is unwind-free; both controls are live"
end
