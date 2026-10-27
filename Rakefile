# frozen_string_literal: true

require "bundler/gem_tasks"
require "rspec/core/rake_task"
require "rake/extensiontask"
require "shellwords"
require "tmpdir"

GEMSPEC = Gem::Specification.load("makiri.gemspec")
# The checkout's root, for the tasks in rakelib/ (Rake loads them after this
# file, in name order), where `__dir__` is rakelib/.
ROOT = __dir__

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
  ext.source_pattern = "{src,nupur}/**/*.{rs,toml}"
end

RSpec::Core::RakeTask.new(:spec)

task default: %i[compile spec]
