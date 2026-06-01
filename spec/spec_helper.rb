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

  # The concurrency suite (spec/threading_spec.rb, tagged :threading) is heavy —
  # 8 threads × 50 iterations under GC.stress — and dominates the local run
  # time. Skip it by default for a fast inner loop. It is REQUIRED in CI
  # (GitHub Actions sets CI=true, and the workflow also sets THREADING=1) and
  # can be forced locally with THREADING=1.
  unless ENV["CI"] || ENV["THREADING"]
    config.filter_run_excluding :threading
    config.before(:suite) do
      warn "[spec] skipping :threading examples (set THREADING=1 to include them)"
    end
  end
end
