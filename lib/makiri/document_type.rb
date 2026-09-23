# frozen_string_literal: true

module Makiri
  # A `<!DOCTYPE ...>` node. Reachable via {Document#internal_subset} or as a
  # child of the document. Exposes the doctype name (via {Node#name}) and its
  # public/system identifiers; XPath cannot reach it (XPath 1.0 has no doctype
  # node type), matching Nokogiri/libxml2.
  class DocumentType < Node
    # `public_id`, `external_id` (Nokogiri-compatible alias), and `system_id`
    # are defined in the extension; each is nil when the identifier is absent,
    # and "" for an empty `PUBLIC ""` literal.
  end
end
