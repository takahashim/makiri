# frozen_string_literal: true

require "makiri"

RSpec.configure do |config|
  # Enable flags like --only-failures and --next-failure
  config.example_status_persistence_file_path = ".rspec_status"

  # Disable RSpec exposing methods globally on `Module` and `main`
  config.disable_monkey_patching!

  config.expect_with :rspec do |c|
    c.syntax = :expect
  end

  # The concurrency suite (spec/threading_spec.rb, tagged :threading) is heavy
  # — many threads under GC.stress — and dominates the run time, so it runs only
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
end
