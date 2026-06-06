# frozen_string_literal: true

require "spec_helper"
require_relative "fuzz/mutate"

# A fast property test over the XML mutation surface: apply many seeded random
# edit sequences to fresh documents and assert every run upholds the structural
# invariants (no cycle, parent/child link consistency, serialization terminates
# and is a stable fixed point under re-parse). The exhaustive run is
# `rake fuzz:mutate` (and it rides `rake fuzz:sanitize` under ASan/UBSan); this is
# the CI-sized gate that keeps the same generator + oracle green on every build.
RSpec.describe "Makiri::XML mutation invariants (property test)" do
  it "upholds the structural invariants across thousands of random edit sequences" do
    findings = []
    3000.times do |seed|
      category, detail = MutateFuzz.run(seed)
      findings << [seed, detail] unless %i[ok expected].include?(category)
    end
    expect(findings).to eq([])
  end
end
