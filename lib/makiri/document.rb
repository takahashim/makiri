# frozen_string_literal: true

module Makiri
  # Abstract base for a parsed document (§12). Concrete documents are the
  # per-kind leaves: {Makiri::HTML::Document} (HTML5) and
  # {Makiri::XML::Document} (XML). `is_a?(Makiri::Document)` is true for both.
  # Construction and the HTML-only conveniences live on the leaves, not here.
  class Document < Node
    # Validate that +document+ is a Makiri::Document and return it, otherwise
    # raise TypeError.
    # @return [Makiri::Document]
    def self.coerce!(document)
      raise TypeError, "expected a Makiri::Document" unless document.is_a?(Makiri::Document)

      document
    end
  end
end
