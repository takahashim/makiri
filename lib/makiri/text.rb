# frozen_string_literal: true

module Makiri
  # A text node.
  class Text < Node
    # Create a detached text node with +content+ owned by +document+
    # (Nokogiri-style constructor; delegates to {Document#create_text_node}).
    #
    # @param content [String]
    # @param document [Makiri::Document]
    # @return [Makiri::Text]
    def self.new(content, document)
      document.create_text_node(content)
    end
  end
end
