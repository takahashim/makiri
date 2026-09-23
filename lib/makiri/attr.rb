# frozen_string_literal: true

module Makiri
  # An attribute node. Most of the API lives in the extension (#name, #value).
  class Attr < Node
    # The element this attribute belongs to, or nil if detached. Defined as a
    # method (not an alias) so it resolves #parent dynamically on the per-kind
    # leaf, where the reader actually lives.
    # @return [Makiri::Element, nil]
    def element
      parent
    end

    private

    XMLNS_NAMESPACE = "http://www.w3.org/2000/xmlns/"
    private_constant :XMLNS_NAMESPACE

    # See {NodePath#path}. An unprefixed attribute name test selects
    # attributes in no namespace; any other, and a name like "xml:lang" or
    # "@click", is written by expanded name. A namespace declaration in the
    # XMLNS namespace is no attribute to XPath at all.
    def path_step
      return if namespace_uri == XMLNS_NAMESPACE

      name_step("@", nil)
    end
  end
end
