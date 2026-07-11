# frozen_string_literal: true

require "makiri"

# GC.auto_compact + GC.stress mode (CI's nightly gc-compact-stress job, set
# GC_COMPACT_STRESS=1). auto_compact is enabled process-wide so Ruby objects
# actually move on every GC; GC.stress is enabled per-example via an around hook
# (process-wide stress makes even loading the spec files run a full GC per
# allocation - tens of minutes before the first example). Under this combination
# every allocation inside an example triggers a *compacting* GC, the strongest
# form of the borrowed-pointer / use-after-move test for the C extension.
GC_COMPACT_STRESS = !ENV["GC_COMPACT_STRESS"].to_s.empty?
GC.auto_compact = true if GC_COMPACT_STRESS

# Iteration count for the high-volume "churn" memory-safety loops (parse/drop
# cycles, index rebuilds). Normally they run at high volume WITHOUT per-allocation
# GC.stress. Under GC_COMPACT_STRESS the around hook forces a compacting GC on
# *every* allocation - orders of magnitude heavier per iteration - so a far
# smaller count exercises the same paths under maximum object movement while
# keeping the nightly job within its time budget. Override the stressed count
# with GC_COMPACT_ITERS to dial the runtime.
def gc_churn_iters(normal, stressed = Integer(ENV.fetch("GC_COMPACT_ITERS", "30")))
  GC_COMPACT_STRESS ? stressed : normal
end

RSpec.configure do |config|
  # Enable flags like --only-failures and --next-failure
  config.example_status_persistence_file_path = ".rspec_status"

  if GC_COMPACT_STRESS
    config.around(:each) do |example|
      GC.stress = true
      begin
        example.run
      ensure
        GC.stress = false
      end
    end
  end

  # Disable RSpec exposing methods globally on `Module` and `main`
  config.disable_monkey_patching!

  config.expect_with :rspec do |c|
    c.syntax = :expect
  end

  # The concurrency suite (spec/threading_spec.rb, tagged :threading) is heavy
  # - many threads under GC.stress - and dominates the run time, so it runs only
  # when THREADING=1 is set explicitly. The CI workflow sets THREADING=1 on a
  # single representative job (the safety it checks is structural, not OS/Ruby
  # specific) rather than on every matrix job. It is NOT auto-enabled by CI=true,
  # so the other matrix jobs (and the local default) skip it for a fast run.
  # (Treat an empty THREADING as unset: the workflow expands the env to "" on
  # the jobs that should skip, and "" is truthy in Ruby.)
  if ENV["THREADING"].to_s.empty?
    config.filter_run_excluding :threading
    config.before(:suite) do
      warn "[spec] skipping :threading examples (set THREADING=1 to include them)"
    end
  end

  # Under Valgrind (VALGRIND=1, set by the spec:valgrind task) skip the examples
  # tagged :slow. These are fail-closed *limit* tests that push a budget to its
  # cap by sheer volume (e.g. registering 70k namespaces) - the memory operations
  # they exercise are identical to the smaller tests, so they add no memory-safety
  # coverage under memcheck, but at ~10-50x slowdown they dominate the run (and,
  # being single examples, can't be split across the sharded jobs). The normal CI
  # matrix still runs them for correctness.
  unless ENV["VALGRIND"].to_s.empty?
    config.filter_run_excluding :slow
    config.before(:suite) { warn "[spec] skipping :slow examples under Valgrind" }
  end
end
