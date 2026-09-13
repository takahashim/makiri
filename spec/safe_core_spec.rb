# frozen_string_literal: true

# The native self-checks: edge and overflow paths that real input cannot reach,
# exercised from inside the extension and reported as one boolean.
#
# `__c_selftest` keeps its name although nothing behind it is C any more. It is
# a documented hook (the sanitizer and Valgrind jobs call it), and renaming it
# would buy nothing.
#
# What it covers shrank when the C was retired, and deliberately so. It used to
# run five checks; two of them - the safe-core primitives and the XML XPath
# round trip - had their subject replaced by something stronger rather than
# removed. The primitives are now proved by Kani (`rake kani`: the allocator,
# the capped buffer, the UTF-8 validator and decoder), which quantifies over
# every input in range instead of sampling a handful; the XML XPath path is
# covered end to end by the query specs through the public API. The three that
# remain are the arena, tree and mutation checks, which reach states no public
# API can construct.
RSpec.describe "native self-checks" do
  it "passes the arena / tree / mutation self-tests" do
    expect(Makiri.__c_selftest).to be(true)
  end
end
