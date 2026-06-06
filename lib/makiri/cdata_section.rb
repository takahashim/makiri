# frozen_string_literal: true

module Makiri
  # A CDATA section. The canonical name is the WHATWG DOM interface name
  # CDATASection; Makiri::CDATA is a Nokogiri-compatible alias (see
  # compat_aliases.rb).
  class CDATASection < Node
    # Create a detached CDATA section owned by +document+ (Nokogiri-style,
    # document first). Delegates to {Document#create_cdata} - so XML only; HTML
    # has no CDATA construction.
    #
    # @param document [Makiri::Document]
    # @param content [String]
    # @return [Makiri::CDATASection]
    def self.new(document, content)
      raise TypeError, "expected a Makiri::Document" unless document.is_a?(Makiri::Document)

      document.create_cdata(content)
    end
  end
end
