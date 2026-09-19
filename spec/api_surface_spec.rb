# frozen_string_literal: true

require_relative "../script/api_manifest"

# The recorded Ruby API surface (see script/api_manifest.rb for what it covers
# and why). This is the one check that does not depend on an example calling
# the method: the rest of the suite catches a broken method, not a missing one.
RSpec.describe "Makiri's API surface" do
  # The lines of `a` that `b` does not have as many of, so a duplicated line
  # (the same method name under several classes) is still reported when one
  # occurrence goes.
  def multiset_diff(a, b)
    left = b.tally
    a.each_with_object([]) do |line, out|
      if left[line].to_i.positive?
        left[line] -= 1
      else
        out << line
      end
    end
  end

  it "matches the recorded manifest" do
    recorded = File.read(ApiManifest::FIXTURE)
    current = ApiManifest.generate

    # Report the difference itself, not two 439-line blobs. The subtraction is
    # a MULTISET one: `def root(0)` occurs in three blocks, and `Array#-` would
    # report a dropped one as no difference at all.
    added = multiset_diff(current.lines, recorded.lines)
    removed = multiset_diff(recorded.lines, current.lines)
    detail = [
      added.empty? ? nil : "added:\n#{added.join}",
      removed.empty? ? nil : "removed:\n#{removed.join}"
    ].compact.join("\n")

    expect(current).to eq(recorded), <<~MSG
      The API surface differs from spec/api_surface.txt.

      #{detail}
      If the change was intended, re-record it in the SAME commit as the change
      (`bundle exec rake api:record`), so it reviews as an API change. If it was
      not, this is the regression the manifest exists to catch.
    MSG
  end
end
