# frozen_string_literal: true

# Phase 1 of the C-safety refactor: the Ruby-free safe primitives
# (ext/makiri/core/mkr_safe.{c,h}) - overflow-checked size arithmetic, the
# capped growable byte buffer, and the pointer vector. The C self-test
# (mkr_safe_selftest) exercises the edge/overflow paths that real inputs cannot
# reach; this spec just runs it.
RSpec.describe "Makiri safe core (mkr_safe)" do
  it "passes the C self-test (size arithmetic / mkr_buf_t / mkr_vec_t)" do
    expect(Makiri.__c_selftest).to be(true)
  end
end
