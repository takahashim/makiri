# frozen_string_literal: true

module Makiri
  # An attribute node. Most of the API lives in C (#name, #value).
  #
  # An attribute knows its owning element through the document's lazily-built
  # attr->owner index, exposed as both #parent and #element.
  class Attribute < Node
    # The element this attribute belongs to, or nil if detached.
    # @return [Makiri::Element, nil]
    alias element parent
  end
end
