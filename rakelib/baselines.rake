# frozen_string_literal: true

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
