# frozen_string_literal: true

module Makiri
  # Abstract base for a parsed document (§12). Concrete documents are the
  # per-kind leaves: {Makiri::HTML::Document} (HTML5) and
  # {Makiri::XML::Document} (XML). `is_a?(Makiri::Document)` is true for both.
  # Construction and the HTML-only conveniences live on the leaves, not here.
  class Document < Node
    # Deprecated shim for the pre-1.0 `Makiri::Document.parse(html)` entry point;
    # parses HTML5. Prefer {Makiri.parse} / {Makiri::HTML::Document.parse}.
    #
    # @param source [String, #read]
    # @return [Makiri::HTML::Document]
    def self.parse(source)
      HTML::Document.parse(source)
    end
  end
end
