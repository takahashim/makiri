# frozen_string_literal: true

module Makiri
  # #clone as "#dup, then honour Ruby's +freeze:+ keyword": +true+ returns a
  # frozen copy, +false+ an unfrozen one, the default (+nil+) copies the
  # receiver's frozen state. Ruby's own #clone allocates and copies, which the
  # native classes cannot do (their allocator is undef'd), so every class with a
  # working #dup gets #clone from here - and a class that changes how it copies
  # overrides #dup alone.
  module CloneViaDup
    def clone(freeze: nil)
      copy = dup
      copy.freeze if freeze || (freeze.nil? && frozen?)
      copy
    end
  end
  # Mixed in by Makiri's own classes; not API.
  private_constant :CloneViaDup
end
