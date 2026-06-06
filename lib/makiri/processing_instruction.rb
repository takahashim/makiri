# frozen_string_literal: true

module Makiri
  # An XML/HTML processing-instruction node. Rare in HTML5 (the parser usually
  # treats "<?...>" as a bogus comment), present mainly for completeness.
  class ProcessingInstruction < Node
    # Create a detached processing instruction owned by +document+ (Nokogiri-style,
    # document first). Delegates to {Document#create_processing_instruction}.
    #
    # @param document [Makiri::Document]
    # @param target [String]
    # @param content [String]
    # @return [Makiri::ProcessingInstruction]
    def self.new(document, target, content)
      raise TypeError, "expected a Makiri::Document" unless document.is_a?(Makiri::Document)

      document.create_processing_instruction(target, content)
    end
  end
end
